//! fossil-mcp binary — the fossil-graph verb surface as **one MCP tool per
//! verb**, over stdio. `lib.rs` is the native execution core; this is the
//! transport and the configuration around it.
//!
//! ```text
//! fossil-mcp --dataset <path> [--allow-raw-sql]
//! ```
//!
//! # The dataset is server configuration, not a tool argument
//!
//! It used to be a per-call argument, because the one generic tool
//! (`dispatch_verb`) took `{ dataset, operation }` and the caller supplied
//! both. Six typed tools exist so that **a tool's input schema is its verb's
//! `Params`** — that is the whole point of the change, and threading a dataset
//! blob through all six would put a field in every schema that no verb has and
//! that the model has to reconstruct identically six times. So it is read once
//! at startup from the JSON file `--dataset` names: `{ dest, secret?,
//! manifest_files }`, the [`fossil_mcp::Dataset`] shape unchanged.
//!
//! **A path in argv is not a secret in argv.** The invariant is that
//! credentials never ride the argument vector or the environment, both of which
//! are readable by other processes; a side-car file the server opens is the
//! form the surveyed tools use, and it is the same one `fossil run
//! --creds-stdin` honours. Stdin itself is not available here: it is the MCP
//! transport.
//!
//! # `--allow-raw-sql`
//!
//! Off by default. Without it the server publishes **five** tools and `read`
//! refuses a `where`; those two are one decision because both fields carry the
//! same authority — see `fossil_mcp::tools`.

use std::path::PathBuf;
use std::sync::Arc;

use fossil_mcp::tools::{SqlPolicy, VerbSurface};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::stdio,
};

/// The server: a dataset opened per call, and the tool surface a policy chose.
#[derive(Clone)]
struct FossilMcp {
    dataset: Arc<PathBuf>,
    surface: VerbSurface,
    tools: Arc<Vec<Tool>>,
}

impl FossilMcp {
    fn new(dataset: PathBuf, policy: SqlPolicy) -> Self {
        let surface = VerbSurface::new(policy);
        Self {
            dataset: Arc::new(dataset),
            tools: Arc::new(surface.tools()),
            surface,
        }
    }

    /// Read the dataset file for this call.
    ///
    /// Read per call rather than once, so an operator rotating a credential or
    /// re-publishing a manifest does not have to restart the server — and so
    /// the secret is not resident between calls.
    fn dataset(&self) -> Result<fossil_mcp::Dataset, McpError> {
        let text = std::fs::read_to_string(self.dataset.as_ref()).map_err(|e| {
            McpError::internal_error(format!("--dataset {}: {e}", self.dataset.display()), None)
        })?;
        serde_json::from_str(&text)
            .map_err(|e| McpError::internal_error(format!("--dataset: {e}"), None))
    }
}

impl ServerHandler for FossilMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "The fossil corpus this server was started against, as one tool per verb. \
                 Call `schema` first: the other tools take the vertex and edge type names it \
                 returns. Rows come back as JSON objects."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools.as_ref().clone()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let operation = self
            .surface
            .operation(&request.name, request.arguments)
            .map_err(|e| McpError::invalid_params(e, None))?;
        let result = fossil_mcp::dispatch_json(self.dataset()?, operation)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CallToolResult::success(vec![Content::json(result)?]))
    }
}

/// `--dataset <path>` is required; `--allow-raw-sql` is a flag. Hand-rolled
/// because two arguments do not earn an argument-parsing dependency in a crate
/// that links a database.
fn parse_args() -> Result<(PathBuf, SqlPolicy), String> {
    let mut dataset = None;
    let mut policy = SqlPolicy::Withheld;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dataset" => {
                dataset = Some(PathBuf::from(
                    args.next().ok_or("--dataset needs a path")?.as_str(),
                ));
            }
            "--allow-raw-sql" => policy = SqlPolicy::Allowed,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok((dataset.ok_or("--dataset <path> is required")?, policy))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (dataset, policy) = parse_args().map_err(|e| {
        format!("{e}\n\nusage: fossil-mcp --dataset <path to dataset.json> [--allow-raw-sql]")
    })?;
    let service = FossilMcp::new(dataset, policy).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
