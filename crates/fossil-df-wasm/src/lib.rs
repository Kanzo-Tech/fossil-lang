//! `fossil-df-wasm` — the `DataFusion` executor ([`fossil_df`]) exposed to JS.
//!
//! The browser runs the mapping: keasy (the thin server) hands the playground a
//! fossil program + signed source URLs; JS `fetch`es each source's bytes and
//! calls [`FossilExecutor::run`], which builds a `DataFusion` plan
//! (`lower_to_mir_pg` → `execute_graph`), materialises the `GraphAr` graph, and
//! returns the output files (Parquet + manifest YAML, as bytes) plus the
//! `RunStatus`. JS then signed-`PUT`s the files and `PATCH`es the job. No
//! mapping runtime on the server (design §E2).
//!
//! ## Source seam (design §C2)
//! The executor reads sources through the [`SessionContext`]'s object stores.
//! Here the host stages each fetched source's bytes in an
//! [`object_store::memory::InMemory`] store keyed by the source URI's
//! scheme+authority, so the executor's existing `read_csv`/`read_parquet` path
//! reads them unchanged. Swapping `InMemory` for an HTTP/signed-URL store
//! (true streaming, larger-than-RAM) is a host-only change — same seam. RDF
//! sources take the provider seam ([`fossil_df::register_rdf`]) instead.
//!
//! ## Native vs wasm split
//! The pure-Rust [`execute_core`] (target-agnostic — `cargo test` drives it on a
//! tokio runtime) holds all the logic; the `#[wasm_bindgen]` [`FossilExecutor`]
//! wrapper only marshals JS values (and is driven by JS's event loop in the
//! browser, no tokio). Mirrors the `*_native` split in `fossil-wasm`.

// The executor is single-threaded by construction (the browser has no threads;
// native callers drive it on a current-thread runtime), so its futures need not
// be `Send` — and DataFusion's execution futures aren't. The workspace's
// `future_not_send` lint is moot here.
#![allow(clippy::future_not_send)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use datafusion::execution::context::SessionContext;
use datafusion::prelude::SessionConfig;
use fossil_base::{FossilDb, FsError, SourceFile, System};
use fossil_descriptors_output::{OutputDescriptorKind, ShExDescriptor};
use fossil_df::files::GraphArFile;
use fossil_df::SourceFormat;
use fossil_run_status::RunStatus;
use object_store::memory::InMemory;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt};
use url::Url;
use wasm_bindgen::prelude::*;

/// One source the host fetched: its program URI (`io.csv("…")`), its format, and
/// the raw bytes. Object-store formats are staged in an [`InMemory`] store;
/// `Rdf` is decoded through the provider seam.
#[derive(Debug, Clone)]
pub struct SourceInput {
    pub uri: String,
    pub format: SourceKind,
    pub bytes: Vec<u8>,
}

/// The source formats the host knows how to fetch + stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Csv,
    Json,
    Parquet,
    Rdf,
}

impl SourceKind {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            "parquet" => Ok(Self::Parquet),
            "rdf" => Ok(Self::Rdf),
            other => Err(format!("unknown source format `{other}`")),
        }
    }
}

/// The executor result: the `GraphAr` output files (the bytes the host
/// signed-PUTs) and the `RunStatus` keasy turns into DCAT.
#[derive(Debug)]
pub struct ExecOutput {
    pub files: Vec<GraphArFile>,
    pub run_status: RunStatus,
}

/// Minimal [`System`] for the executor host. The executor reads sources through
/// the object-store / provider seams, never through `System::read_file`, and
/// touches no clock on its path — so `read_file` is unreachable (returns
/// `NotFound`) and `now` returns the wasm-safe `UNIX_EPOCH` placeholder
/// (`SystemTime::now()` panics on `wasm32-unknown-unknown`). `inferred_descriptor`
/// stays at the trait default (`None`): typing falls back to the passed output
/// descriptor / string defaults — the descriptor is an argument, not read here.
#[derive(Debug, Default)]
struct ExecutorSystem;

impl System for ExecutorSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound(path.display().to_string()))
    }
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

/// Run a fossil program on `DataFusion` from host-fetched source bytes — the
/// target-agnostic core (`cargo test` drives it natively; the browser drives it
/// via the wasm wrapper). Builds the db + output descriptor, stages each source
/// in the [`SessionContext`], executes the graph, and encodes the `GraphAr` output.
///
/// `dest` is the dataset's logical location (e.g. the job's object-storage
/// prefix) — it only labels the `RunStatus`, no IO happens against it here.
///
/// # Errors
/// `ShEx` parse, URL parse, object-store staging, `DataFusion` execution, or
/// Parquet encode failures — all surfaced as a message string for the JS host.
pub async fn execute_core(
    program: &str,
    shex: Option<&str>,
    sources: Vec<SourceInput>,
    dest: &str,
    connections: &HashMap<String, String>,
) -> Result<ExecOutput, String> {
    let (db, file, descriptor) = build_program(program, shex)?;

    // Single partition: no RepartitionExec, no detached `tokio::spawn` — the
    // whole plan is drivable by one top-level future, which is what makes
    // DataFusion run under wasm-bindgen-futures (no tokio runtime in the browser).
    let config = SessionConfig::new().with_target_partitions(1);
    let ctx = SessionContext::new_with_config(config);

    register_object_store_sources(&ctx, &sources).await?;
    register_rdf_sources(&ctx, &db, file, &descriptor, &sources, connections)?;

    // The executor resolves each `@conn` source alias through `connections`
    // (same name→URL map the browser used in `sources()`), so the registered
    // object-store / RDF tables line up with what the plan reads.
    let graph = fossil_df::execute_graph(&ctx, &db, file, &descriptor, connections)
        .await
        .map_err(|e| format!("execute_graph: {e}"))?;

    let files = graph.to_files().map_err(|e| format!("encode: {e}"))?;
    let run_status = graph.run_status(dest);
    Ok(ExecOutput { files, run_status })
}

/// Enumerate the program's sources as `(uri, format-kind)` — the target-agnostic
/// core behind [`FossilExecutor::sources`]. Pure (no IO); the host uses it to
/// plan its fetches before [`execute_core`]. `format` is the fetch-strategy
/// string (`"csv"`/`"json"`/`"parquet"`/`"rdf"`).
///
/// # Errors
/// `ShEx` parse failures.
pub fn program_sources_core(
    program: &str,
    shex: Option<&str>,
    connections: &HashMap<String, String>,
) -> Result<Vec<(String, &'static str)>, String> {
    let (db, file, descriptor) = build_program(program, shex)?;
    Ok(fossil_df::program_sources(&db, file, &descriptor, connections)
        .into_iter()
        .map(|s| (s.uri, format_kind(&s.format)))
        .collect())
}

/// Build the executor's db + interned program + output descriptor — shared by
/// [`execute_core`] and [`program_sources_core`].
fn build_program(
    program: &str,
    shex: Option<&str>,
) -> Result<(FossilDb, SourceFile, OutputDescriptorKind), String> {
    let descriptor = match shex {
        Some(text) => OutputDescriptorKind::ShEx(
            ShExDescriptor::from_reader(text.as_bytes())
                .map_err(|e| format!("ShEx parse error: {e:?}"))?,
        ),
        None => OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
    };
    let system: Arc<dyn System> = Arc::new(ExecutorSystem);
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), "program.fossil".to_string());
    Ok((db, file, descriptor))
}

/// The host fetch-strategy string for a source format.
const fn format_kind(f: &SourceFormat) -> &'static str {
    match f {
        SourceFormat::Csv => "csv",
        SourceFormat::Json => "json",
        SourceFormat::Parquet => "parquet",
        SourceFormat::Provider { .. } => "rdf",
    }
}

/// Stage every object-store source (`csv`/`json`/`parquet`) in an [`InMemory`]
/// store keyed by its URI's scheme+authority, then register the stores in `ctx`
/// so the executor's `read_csv`/`read_json`/`read_parquet` path reaches them.
async fn register_object_store_sources(
    ctx: &SessionContext,
    sources: &[SourceInput],
) -> Result<(), String> {
    let mut stores: HashMap<String, Arc<InMemory>> = HashMap::new();
    for src in sources {
        if src.format == SourceKind::Rdf {
            continue;
        }
        let url = Url::parse(&src.uri).map_err(|e| format!("source URI `{}`: {e}", src.uri))?;
        let base = base_url(&url)?;
        let store = stores.entry(base).or_insert_with(|| Arc::new(InMemory::new()));
        let path = ObjPath::from(url.path().trim_start_matches('/'));
        store
            .put(&path, src.bytes.clone().into())
            .await
            .map_err(|e| format!("stage `{}`: {e}", src.uri))?;
    }
    for (base, store) in stores {
        let url = Url::parse(&base).map_err(|e| e.to_string())?;
        ctx.register_object_store(&url, store as Arc<dyn ObjectStore>);
    }
    Ok(())
}

/// Decode + register every provider (RDF) source through
/// [`fossil_df::register_rdf`] — match each program-derived provider binding to
/// the host-fetched bytes by URI, exactly the browser counterpart of
/// `fossil_df::register_provider_sources` (which reads bytes from the fs).
fn register_rdf_sources(
    ctx: &SessionContext,
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    sources: &[SourceInput],
    connections: &HashMap<String, String>,
) -> Result<(), String> {
    for binding in fossil_df::provider_bindings(db, file, descriptor, connections) {
        let src = sources
            .iter()
            .find(|s| s.format == SourceKind::Rdf && s.uri == binding.uri)
            .ok_or_else(|| format!("RDF source `{}` was not provided", binding.uri))?;
        let turtle = std::str::from_utf8(&src.bytes)
            .map_err(|e| format!("RDF source `{}` is not UTF-8: {e}", binding.uri))?;
        fossil_df::register_rdf(ctx, &binding, turtle)
            .map_err(|e| format!("register RDF `{}`: {e}", binding.uri))?;
    }
    Ok(())
}

/// The `scheme://authority` key an object store is registered under (datafusion
/// looks up the store by a URL's scheme + authority; the path selects the object).
fn base_url(u: &Url) -> Result<String, String> {
    let authority = u.authority();
    if authority.is_empty() {
        return Err(format!(
            "source URI `{u}` has no host — object-store sources need an absolute URL"
        ));
    }
    Ok(format!("{}://{}", u.scheme(), authority))
}

// ───────────────────────── wasm-bindgen surface ─────────────────────────────

/// JS-facing handle for the browser executor. One instance per tab; cheap to
/// construct (the heavy state — db, ctx — lives per `run` call).
#[wasm_bindgen]
#[derive(Debug, Default)]
pub struct FossilExecutor;

#[wasm_bindgen]
impl FossilExecutor {
    /// Construct an executor, installing the panic hook so a Rust panic surfaces
    /// as a `console.error` stack trace instead of an opaque `unreachable`.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        Self
    }

    /// Enumerate the program's sources so the host knows what to fetch + how to
    /// stage. Returns a JS array of `{ uri, format }`, `format` ∈
    /// `"csv"`/`"json"`/`"parquet"`/`"rdf"`. Pure (no IO) — call it first, fetch
    /// each `uri` by signed URL, then pass the bytes to [`Self::run`].
    ///
    /// `refs` is the connection ref-map `{ name: baseUrl }` (the host's
    /// connections): a `@name/path` source alias resolves to `{baseUrl}/path`.
    /// Pass the SAME `refs` to [`Self::run`] so the resolved URIs line up.
    ///
    /// # Errors
    /// A JS `Error` if the `ShEx` schema fails to parse.
    pub fn sources(
        &self,
        program: String,
        shex: Option<String>,
        refs: JsValue,
    ) -> Result<JsValue, JsError> {
        let connections = parse_refs(&refs).map_err(|e| JsError::new(&e))?;
        let srcs = program_sources_core(&program, shex.as_deref(), &connections)
            .map_err(|e| JsError::new(&e))?;
        let arr = js_sys::Array::new();
        for (uri, format) in srcs {
            let obj = js_sys::Object::new();
            set(&obj, "uri", &JsValue::from_str(&uri)).map_err(|e| JsError::new(&e))?;
            set(&obj, "format", &JsValue::from_str(format)).map_err(|e| JsError::new(&e))?;
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Execute `program` against the host-fetched `sources` and return
    /// `{ files: [{ path, bytes: Uint8Array }], runStatus }`.
    ///
    /// `sources` is a JS array of `{ uri, format, bytes: Uint8Array }` (the `uri`
    /// being the RESOLVED one [`Self::sources`] returned); `format` is
    /// `"csv"`/`"json"`/`"parquet"`/`"rdf"`. `shex` is the optional output schema
    /// text. `refs` is the same `{ name: baseUrl }` map passed to `sources`.
    ///
    /// # Errors
    /// A JS `Error` carrying the message of any parse / staging / execution /
    /// encode failure.
    pub async fn run(
        &self,
        program: String,
        shex: Option<String>,
        sources: JsValue,
        dest: String,
        refs: JsValue,
    ) -> Result<JsValue, JsError> {
        let sources = parse_sources(&sources).map_err(|e| JsError::new(&e))?;
        let connections = parse_refs(&refs).map_err(|e| JsError::new(&e))?;
        let out = execute_core(&program, shex.as_deref(), sources, &dest, &connections)
            .await
            .map_err(|e| JsError::new(&e))?;
        out_to_js(&out).map_err(|e| JsError::new(&e))
    }
}

/// Parse the JS `refs` object `{ name: baseUrl }` into the connection ref-map.
/// `undefined`/`null` (no connections) yields an empty map — all source URIs are
/// then treated as already-concrete.
fn parse_refs(refs: &JsValue) -> Result<HashMap<String, String>, String> {
    if refs.is_undefined() || refs.is_null() {
        return Ok(HashMap::new());
    }
    let obj: &js_sys::Object = refs
        .dyn_ref::<js_sys::Object>()
        .ok_or("`refs` must be an object of { name: baseUrl }")?;
    let mut map = HashMap::new();
    for entry in js_sys::Object::entries(obj).iter() {
        let pair: js_sys::Array = entry.into();
        let name = pair
            .get(0)
            .as_string()
            .ok_or("`refs` keys must be strings")?;
        let url = pair
            .get(1)
            .as_string()
            .ok_or("`refs` values must be strings")?;
        map.insert(name, url);
    }
    Ok(map)
}

/// Parse the JS `sources` array into [`SourceInput`]s, reading each `bytes`
/// `Uint8Array` directly (no serde round-trip — bulletproof for typed arrays).
fn parse_sources(sources: &JsValue) -> Result<Vec<SourceInput>, String> {
    let arr: &js_sys::Array = sources
        .dyn_ref::<js_sys::Array>()
        .ok_or("`sources` must be an array")?;
    let mut out = Vec::with_capacity(arr.length() as usize);
    for item in arr.iter() {
        let uri = get_string(&item, "uri")?;
        let format = SourceKind::parse(&get_string(&item, "format")?)?;
        let bytes_val =
            js_sys::Reflect::get(&item, &JsValue::from_str("bytes")).map_err(|e| js_err(&e))?;
        let bytes = bytes_val
            .dyn_ref::<js_sys::Uint8Array>()
            .ok_or("source `bytes` must be a Uint8Array")?
            .to_vec();
        out.push(SourceInput { uri, format, bytes });
    }
    Ok(out)
}

/// Marshal [`ExecOutput`] to `{ files: [{ path, bytes: Uint8Array }], runStatus }`.
/// Bytes go out as `Uint8Array` (not a number array) so large Parquet payloads
/// stay zero-copy-ish on the JS side.
fn out_to_js(out: &ExecOutput) -> Result<JsValue, String> {
    let files = js_sys::Array::new();
    for f in &out.files {
        let obj = js_sys::Object::new();
        set(&obj, "path", &JsValue::from_str(&f.rel_path))?;
        set(&obj, "bytes", &js_sys::Uint8Array::from(f.bytes.as_slice()))?;
        files.push(&obj);
    }
    let run_status = serde_wasm_bindgen::to_value(&out.run_status).map_err(|e| e.to_string())?;
    let result = js_sys::Object::new();
    set(&result, "files", &files)?;
    set(&result, "runStatus", &run_status)?;
    Ok(result.into())
}

fn get_string(obj: &JsValue, key: &str) -> Result<String, String> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .map_err(|e| js_err(&e))?
        .as_string()
        .ok_or_else(|| format!("source field `{key}` must be a string"))
}

fn set(obj: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), String> {
    js_sys::Reflect::set(obj, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|e| js_err(&e))
}

fn js_err(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| "JS error".to_string())
}
