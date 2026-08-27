//! fossil-mcp binary — exposes the fossil-graph verb surface as an MCP tool
//! over stdio. The host (keasy) spawns this as a subprocess and calls the
//! `dispatch_verb` tool; see `lib.rs` for the native execution core.
//!
//! One generic tool (`dispatch_verb`) covers all six verbs via the
//! `{ verb, params }` operation envelope — the minimal surface that is still
//! the full verb service. Per-verb LLM-facing tools (for the `/ask` tool-loop)
//! are a follow-up; the transport + execution core land here.

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;

#[derive(Clone)]
struct FossilMcp {
    tool_router: ToolRouter<Self>,
}

/// `dispatch_verb` arguments. `dataset` + `operation` are opaque JSON here
/// (parsed in the handler) so the tool schema stays decoupled from
/// `fossil-graph`'s own `schemars` version.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DispatchVerbParams {
    /// `{ dest, secret?, manifest_files }` — see `fossil_mcp::Dataset`.
    dataset: serde_json::Value,
    /// `{ "verb": "...", "params": {...} }` — the fossil-graph operation envelope.
    operation: serde_json::Value,
}

#[tool_router]
impl FossilMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Dispatch a fossil-graph verb against a GraphAr dataset. \
        `operation` is {verb,params}; `dataset` is {dest,secret?,manifest_files}. \
        Returns the verb's JSON result."
    )]
    async fn dispatch_verb(
        &self,
        Parameters(p): Parameters<DispatchVerbParams>,
    ) -> Result<CallToolResult, McpError> {
        let dataset: fossil_mcp::Dataset = serde_json::from_value(p.dataset)
            .map_err(|e| McpError::invalid_params(format!("dataset: {e}"), None))?;
        // The generic tool grants raw SQL, because the generic tool has no way
        // not to: one tool cannot be half-registered. Making that a policy is
        // what the six typed tools are for — `fossil_graph::raw_sql`.
        let operation = fossil_graph::Operation::from_wire(
            &p.operation,
            Some(fossil_graph::RawSqlAccess::granted()),
        )
        .map_err(|e| McpError::invalid_params(format!("operation: {e}"), None))?;
        let result = fossil_mcp::dispatch_json(dataset, operation)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CallToolResult::success(vec![Content::json(result)?]))
    }
}

#[tool_handler]
impl ServerHandler for FossilMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "fossil-graph verb surface over MCP. Call `dispatch_verb` with a \
                 dataset context and a {verb,params} operation."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = FossilMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
