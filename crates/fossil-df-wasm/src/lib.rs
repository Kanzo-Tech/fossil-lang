//! `fossil-df-wasm` — the `DataFusion` executor ([`fossil_df`]) exposed to JS.
//!
//! The browser runs the mapping. An [`Executor`] holds one compiled program and
//! is a document workspace: it reports the documents the program names and does
//! not hold ([`Executor::missing_documents`]), the host reads them through its
//! `SourceHost` and registers each one, and only then are the sources listed
//! and the run executed — against the output descriptor decoded from those
//! registered documents, which is what the checker read. [`Executor::execute`]
//! builds a `DataFusion` plan (`lower_to_mir_pg` → `execute_graph`),
//! materialises the `GraphAr` graph, and returns the output files (Parquet +
//! manifest YAML, as bytes) plus the [`RunReport`] — the same manifest, as
//! JSON, so the host does not parse back out of the bytes it is about to
//! upload. No mapping runtime on the server.
//!
//! ## Source seam
//! The executor reads sources through the [`SessionContext`]'s object stores.
//! Here the host stages each fetched source's bytes in an
//! [`object_store::memory::InMemory`] store keyed by the source URI's
//! scheme+authority, so the executor's existing `read_csv`/`read_parquet` path
//! reads them unchanged. Swapping `InMemory` for an HTTP/signed-URL store
//! (true streaming, larger-than-RAM) is a host-only change — same seam. RDF
//! sources take the provider seam ([`fossil_df::register_rdf`]) instead.
//!
//! ## Native vs wasm split
//! The pure-Rust [`Executor`] (target-agnostic — `cargo test` drives it on a
//! tokio runtime) holds all the logic; the `#[wasm_bindgen]` [`FossilExecutor`]
//! wrapper only marshals JS values (and is driven by JS's event loop in the
//! browser, no tokio). Mirrors `fossil-wasm`'s `FossilPlayground` split.

// The executor is single-threaded by construction (the browser has no threads;
// native callers drive it on a current-thread runtime), so its futures need not
// be `Send` — and DataFusion's execution futures aren't. The workspace's
// `future_not_send` lint is moot here.
#![allow(clippy::future_not_send)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use datafusion::execution::context::SessionContext;
use datafusion::prelude::SessionConfig;
use fossil_base::{FossilDb, FsError, Provider, RowReader, SourceFile, System};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_df::RunReport;
use fossil_df::SourceFormat;
use fossil_df::files::GraphArFile;
use fossil_hir::documents::MissingDocument;
use fossil_layout::io::MemoryFs;
use object_store::memory::InMemory;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt};
use url::Url;
use wasm_bindgen::prelude::*;

/// One source the host fetched: its program URI (`io.csv("…")`), the catalogue
/// ROW it was written with, and the raw bytes.
///
/// # `format` is a catalogue row and not an enum of its own
///
/// The one question this crate asks of it is *does this go through the object
/// store or through the provider seam* — [`RowReader::Native`] against
/// [`RowReader::Materialised`], a property the row already carries. Asking the
/// row means a second materialised provider works here the day it is a line in
/// `catalogue.bnf`, instead of being silently staged as bytes for a reader that
/// cannot read it.
#[derive(Debug, Clone)]
pub struct SourceInput {
    pub uri: String,
    pub format: &'static Provider,
    pub bytes: Vec<u8>,
}

/// The row a wire `format` string names, which must be one that reads DATA.
///
/// [`DATA`](fossil_base::providers::DATA) rather than the full table: a host
/// fetching bytes for `io.shex` would be a program that got past the checker,
/// and the wire has no business naming a row that decodes types.
///
/// Public because a caller building a [`SourceInput`] needs one.
///
/// # Errors
///
/// When no data row is called `name`.
pub fn source_row(name: &str) -> Result<&'static Provider, String> {
    fossil_base::provider(fossil_base::providers::DATA, name)
        .ok_or_else(|| format!("unknown source format `{name}`"))
}

/// Does this row's bytes get staged in the object store for a native reader to
/// scan? The complement is the provider seam — see [`SourceInput`].
const fn is_object_store(row: &Provider) -> bool {
    matches!(row.reads_rows, Some(RowReader::Native(_)))
}

/// Does this row's relation get materialised outside the reader?
const fn is_materialised(row: &Provider) -> bool {
    matches!(row.reads_rows, Some(RowReader::Materialised))
}

/// The executor result: the `GraphAr` output files (the bytes the host
/// signed-PUTs) and the [`RunReport`] keasy turns into DCAT.
///
/// The report duplicates nothing in `files`: the manifest YAMLs are in there as
/// bytes, and this is the same values already parsed. A host that only wants to
/// upload can ignore it; one that wants to know what it uploaded would otherwise
/// have to YAML-parse its own payload.
#[derive(Debug)]
pub struct ExecOutput {
    pub files: Vec<GraphArFile>,
    pub report: RunReport,
}

/// Minimal [`System`] for the executor host. The executor reads sources through
/// the object-store / provider seams and documents through the registry, never
/// through `System::read_file`, and touches no clock on its path — so
/// `read_file` is unreachable (returns `NotFound`) and `now` returns the
/// wasm-safe `UNIX_EPOCH` placeholder (`SystemTime::now()` panics on
/// `wasm32-unknown-unknown`).
///
/// `providers` installs the type-reading rows: a mapping header names a bare
/// type bound positionally by `type { … } := io.shex("…")`, so the vertex label
/// and every predicate come from the registered document, decoded by its row.
#[derive(Debug, Default)]
struct ExecutorSystem;

impl System for ExecutorSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotFound(path.display().to_string()))
    }
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
    fn providers(&self) -> &'static [&'static Provider] {
        fossil_descriptors_output::PROVIDERS
    }
}

/// One compiled program and the documents registered for it — the
/// target-agnostic core behind [`FossilExecutor`].
///
/// The order is the native host's and it is forced: parse → ask which
/// documents the program names → register them → list sources and run. The
/// output descriptor is decoded from the registry on each call, so it is always
/// the document the checker read.
pub struct Executor {
    db: FossilDb,
    file: SourceFile,
    connections: HashMap<String, String>,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("connections", &self.connections)
            .finish_non_exhaustive()
    }
}

impl Executor {
    /// Compile `program`, with no connections until [`Self::set_connections`].
    #[must_use]
    pub fn new(program: &str) -> Self {
        let system: Arc<dyn System> = Arc::new(ExecutorSystem);
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, program.to_string(), "program.fossil".to_string());
        Self {
            db,
            file,
            connections: HashMap::new(),
        }
    }

    /// The map (`{ name: baseUrl }`) every `@name/…` a document or source names
    /// expands against. It moves locators, never registry keys.
    // The hasher is not ours to choose: the map arrives from
    // `parse_connections(&JsValue)`, which builds a plain `HashMap`.
    #[allow(clippy::implicit_hasher)]
    pub fn set_connections(&mut self, connections: HashMap<String, String>) {
        self.connections = connections;
    }

    /// The documents the program names that are not registered yet.
    #[must_use]
    pub fn missing_documents(&self) -> Vec<MissingDocument> {
        fossil_hir::documents::missing_documents(&self.db, self.file, &self.connections)
    }

    /// Register a fetched document under the key [`Self::missing_documents`]
    /// reported for it.
    pub fn register_document(&mut self, key: &str, text: &str) {
        fossil_base::register_document(&mut self.db, key, text);
    }

    /// The program's sources as `(locator, row-name)`: what the host signs and
    /// fetches before [`Self::execute`]. The second element is the catalogue
    /// row's name, which is what the program wrote after `io.` and what the host
    /// hands back on `SourceInput.format`.
    ///
    /// # Errors
    /// The output shape document is unregistered or does not decode.
    pub fn sources(&self) -> Result<Vec<(String, String)>, String> {
        let descriptor = self.descriptor()?;
        Ok(
            fossil_df::program_sources(&self.db, self.file, &descriptor, &self.connections)
                .into_iter()
                .map(|s| (s.uri, format_kind(&s.format).to_owned()))
                .collect(),
        )
    }

    fn descriptor(&self) -> Result<OutputDescriptorKind, String> {
        fossil_df::output_descriptor(&self.db, self.file)
    }

    /// Run the program on `DataFusion` from host-fetched source bytes: stage each
    /// source in the [`SessionContext`], execute the graph, and encode the
    /// `GraphAr` output.
    ///
    /// `dest` is the dataset's logical location (e.g. the job's object-storage
    /// prefix) — it only labels the [`RunReport`], no IO happens against it here.
    ///
    /// # Errors
    /// Output shape, URL parse, object-store staging, `DataFusion` execution, or
    /// Parquet encode failures — all surfaced as a message string for the JS host.
    pub async fn execute(
        &self,
        sources: Vec<SourceInput>,
        dest: &str,
    ) -> Result<ExecOutput, String> {
        let (db, file, connections) = (&self.db, self.file, &self.connections);
        let descriptor = self.descriptor()?;

        // Single partition: no RepartitionExec, no detached `tokio::spawn` — the
        // whole plan is drivable by one top-level future, which is what makes
        // DataFusion run under wasm-bindgen-futures (no tokio runtime in the browser).
        let config = SessionConfig::new().with_target_partitions(1);
        let ctx = SessionContext::new_with_config(config);

        register_object_store_sources(&ctx, &sources).await?;
        register_rdf_sources(&ctx, db, file, &descriptor, &sources, connections)?;

        // The executor resolves each `@conn` source alias through `connections`
        // (same name→URL map the browser used in `sources()`), so the registered
        // object-store / RDF tables line up with what the plan reads.
        let mut graph = fossil_df::execute_graph(&ctx, db, file, &descriptor, connections)
            .await
            .map_err(|e| format!("execute_graph: {e}"))?;

        // **The payload first, then the manifests over it** — the order `fossil run`
        // writes in, and for the same reason: a document written before the bytes
        // can only promise them. `enrich_layout_in_memory` hands back the filesystem
        // it wrote into, and the manifests go in on top.
        let (fs, layout) = enrich_layout_in_memory(&graph)?;
        graph.declare_pyramids(layout.pyramids);
        // Both measured fields, and both by the same argument the native host makes:
        // a rung's quotient and a categorical channel's domain are numbers that do
        // not exist until the pass has run. The tab declares them because the tab
        // ran the pass — see this function's doc for why it is not allowed to skip.
        graph.declare_channels(layout.channels);
        // **The report is built here — after every declaration — and the position of
        // this line is the whole of what it says.** [`RunReport::of`] takes a
        // snapshot of `graph.manifest()`, so a report built earlier states the
        // manifest as it was at that moment and says nothing about a field declared
        // after it: not an error, not a `None` a reader can interrogate, just a key
        // that is absent from the JSON while it is present in the YAML the very same
        // call emits below. It WAS built first, and the browser's account of a run
        // silently lost `cells:` and then `channels:` — two measured facts about a
        // corpus the tab had just written, missing from the tab's own report of
        // writing it, while `fossil run --output-json` carried both.
        //
        // `fossil-cli`'s `host::run` builds its report in the same place for the same
        // reason, and `/docs/design/three-hosts` is the page that says the two hosts
        // differ only in their `System`.
        // `tests/execute_core.rs::the_report_is_the_manifest_the_browser_shipped`
        // holds the report against the emitted YAML so a third field cannot repeat it.
        let report = RunReport::of(dest, &graph);
        for file in graph.manifest_files().map_err(|e| format!("encode: {e}"))? {
            fs.insert(file.rel_path, file.bytes);
        }
        let files = fs
            .drain()
            .into_iter()
            .map(|(rel_path, bytes)| GraphArFile {
                rel_path,
                bytes: bytes.to_vec(),
            })
            .collect();
        Ok(ExecOutput { files, report })
    }
}

/// Run the real layout pass, in memory, and hand back the filesystem it wrote
/// the payload into.
///
/// This is `fossil_cli::host::enrich_written_layout` with a different
/// filesystem under it, and the two are deliberately the same shape: the same
/// targets, the same adjacencies in both orientations. **What the tab writes has
/// to be what `fossil run` writes**, and the only way to be sure of that is for
/// the browser to run the pass rather than to approximate it.
///
/// It is not optional, and it is not a rewrite: `fossil-df` produces
/// `x`/`y`/`cluster_id` as zeroed placeholders in batches nobody has written
/// yet, so this pass is what puts the payload in the map at all. A browser that
/// skipped it would publish manifests over an empty tree.
///
/// What it costs: one resident copy of every file the pass writes, because there
/// is no filesystem here to stream to as the native pass does. The pass is the
/// memory-hungry half of the write path (`/docs/design/streaming`) and
/// `wasm32`'s address space is 4 GiB, so a corpus that fits natively can fail
/// here. That ceiling is a property of the target, stated rather than worked
/// around.
///
/// It used to be **three** resident copies at the crossing point: the executor's
/// Arrow, the staged Parquet this function inserted into the map, and the copy
/// the pass decoded back out of it. Two of the three were there so that a pass
/// in the same process could read what the same process had just encoded.
/// It hands back the whole [`LayoutReport`] and not one field of it, because
/// there are two measured fields now and a second one destructured at the call
/// site is a second chance for this host to drop one the native host declares.
fn enrich_layout_in_memory(
    graph: &fossil_df::GraphArData,
) -> Result<(MemoryFs, fossil_layout::layout::LayoutReport), String> {
    use fossil_layout::layout::{AdjacencyTarget, Endpoint, VertexLayoutTarget};

    // No `dest` prefix: the native host joins one because it writes into a
    // directory, and this writes into a map whose keys are what JS receives.
    let fs = MemoryFs::new();

    let relation =
        |e: &fossil_df::EdgeTable| format!("edge/{}_{}_{}/", e.src_type, e.label, e.dst_type);

    // `graph.vertices` and not `graph.schema.nodes`: this is the side that
    // carries the rows, and the native host reads the same one for the same
    // reason.
    let targets: Vec<VertexLayoutTarget<'_>> = graph
        .vertices
        .iter()
        .map(|table| VertexLayoutTarget {
            type_name: table.label.clone(),
            batches: &table.batches,
            // Trailing separator: the layout appends the tiles file.
            chunk_prefix: format!("vertex/{}/", table.label),
            // The same constant the manifest is written with, so the files and
            // the promise cannot drift apart.
            chunk_size: fossil_sinks::manifest::DEFAULT_CHUNK_SIZE,
            // The same base the native host declares. **What the tab writes has
            // to be what `fossil run` writes**, and a pyramid the browser
            // skipped would be a corpus the two paths disagree about.
            vertices_per_cell: Some(fossil_sinks::manifest::DEFAULT_VERTICES_PER_CELL),
        })
        .collect();

    // Every orientation, cross-type included. The pass renumbers `dense_id`, so
    // one left out keeps ids that now belong to somebody else — a silent
    // corruption, which is why this enumerates rather than letting the layout
    // guess.
    let adjacencies: Vec<AdjacencyTarget<'_>> = graph
        .edges
        .iter()
        .flat_map(|e| {
            [
                ("by_source", Endpoint::Src, &e.by_source),
                ("by_target", Endpoint::Dst, &e.by_target),
            ]
            .map(|(dir, ordered_by, batches)| AdjacencyTarget {
                src_type: e.src_type.clone(),
                label: e.label.clone(),
                dst_type: e.dst_type.clone(),
                ordered_by,
                batches,
                tile_prefix: format!("{}{dir}/", relation(e)),
                levels_prefix: relation(e),
            })
        })
        .collect();

    let report = fossil_layout::layout::enrich_layout_with(&fs, &targets, &adjacencies)
        .map_err(|e| format!("layout: {e}"))?;

    Ok((fs, report))
}

/// The host fetch-strategy string for a source format — **the catalogue row's
/// name**, which is what a program wrote after `io.` and what [`source_row`]
/// reads back off the wire.
///
/// The `Provider` arm carries the name rather than the literal `"rdf"` because
/// more than one row can be materialised: labelling an `io.avro` source `rdf`
/// would send it back as the RDF row. That `rdf` is the only materialised row
/// today is what would make the literal correct by coincidence.
fn format_kind(f: &SourceFormat) -> &str {
    match f {
        // `{ .. }` because `Csv` carries the `delimiter =` the program wrote.
        // It is not in the wire string on purpose: this names the FETCH
        // STRATEGY the host has to stage bytes for, and the delimiter is read
        // by `fossil_df::read_source` off the MIR the executor already holds.
        SourceFormat::Csv { .. } => "csv",
        SourceFormat::Json => "json",
        SourceFormat::Parquet => "parquet",
        SourceFormat::Provider { name } => name.as_str(),
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
        if !is_object_store(src.format) {
            continue;
        }
        let url = Url::parse(&src.uri).map_err(|e| format!("source URI `{}`: {e}", src.uri))?;
        let base = base_url(&url)?;
        let store = stores
            .entry(base)
            .or_insert_with(|| Arc::new(InMemory::new()));
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
/// the host-fetched bytes by URI.
///
/// It is the browser counterpart of `fossil_df::register_provider_sources`,
/// which walks the same `provider_bindings` and calls the same `register_rdf`;
/// where the bytes come from is the whole difference, and it gives this side one
/// failure the native side cannot have — a binding the host did not provide.
///
/// **"Exactly the counterpart" is a reading, not a result.** Nothing here
/// drives an RDF source through both halves and compares — `tests/` is CSV — so
/// the two can drift and only a program would notice.
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
            .find(|s| is_materialised(s.format) && s.uri == binding.uri)
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

/// The JS-facing executor — `FossilExecutor` on the JS side, a `RefCell`
/// around the Rust [`Executor`], for the reason `fossil-wasm`'s
/// `WasmPlayground` gives: a `&mut self` export borrows wasm-bindgen's cell
/// exclusively, a failed borrow panics, and on `wasm32` that poisons the object
/// for good. Every export takes `&self`; a register while a run is in flight
/// returns a catchable `Error` instead.
#[wasm_bindgen(js_name = FossilExecutor)]
#[derive(Debug)]
pub struct FossilExecutor {
    inner: RefCell<Executor>,
}

#[wasm_bindgen(js_class = FossilExecutor)]
impl FossilExecutor {
    /// Compile `program`, installing the panic hook so a Rust panic surfaces as
    /// a `console.error` stack trace instead of an opaque `unreachable`.
    // `wasm_bindgen` dictates these by value: it owns the JS→Rust conversion
    // and cannot hand out borrows across the boundary.
    #[wasm_bindgen(constructor)]
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(program: String) -> Self {
        console_error_panic_hook::set_once();
        Self {
            inner: RefCell::new(Executor::new(&program)),
        }
    }

    /// `{ name: baseUrl }` — what `@name/…` expands against.
    /// `@fossil-lang/types`' `resolveDocuments` sets it from the host.
    ///
    /// # Errors
    /// A JS `Error` if `connections` is not an object of strings, or a run is
    /// in flight.
    #[wasm_bindgen(js_name = setConnections)]
    pub fn set_connections(&self, connections: &JsValue) -> Result<(), JsError> {
        let connections = parse_connections(connections).map_err(|e| JsError::new(&e))?;
        self.borrow_mut("setConnections")?
            .set_connections(connections);
        Ok(())
    }

    /// `[{ key, locator }]` — the documents the program names and the executor
    /// does not hold. `@fossil-lang/types`' `resolveDocuments` reads them.
    ///
    /// # Errors
    /// A JS `Error` if a register is in flight.
    #[wasm_bindgen(js_name = missingDocuments)]
    pub fn missing_documents(&self) -> Result<JsValue, JsError> {
        let arr = js_sys::Array::new();
        for missing in self.borrow()?.missing_documents() {
            let obj = js_sys::Object::new();
            set(&obj, "key", &JsValue::from_str(&missing.key)).map_err(|e| JsError::new(&e))?;
            set(&obj, "locator", &JsValue::from_str(&missing.locator))
                .map_err(|e| JsError::new(&e))?;
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Register a fetched document under the `key` `missingDocuments` gave it.
    ///
    /// # Errors
    /// A JS `Error` if a run is in flight.
    #[wasm_bindgen(js_name = registerDocument)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn register_document(&self, key: String, text: String) -> Result<(), JsError> {
        self.borrow_mut("registerDocument")?
            .register_document(&key, &text);
        Ok(())
    }

    /// `[{ uri, format }]` — the sources to sign and fetch, `uri` being the
    /// locator fossil resolved and `format` the catalogue row's name.
    ///
    /// # Errors
    /// A JS `Error` if the output shape document is unregistered or does not
    /// decode.
    pub fn sources(&self) -> Result<JsValue, JsError> {
        let srcs = self.borrow()?.sources().map_err(|e| JsError::new(&e))?;
        let arr = js_sys::Array::new();
        for (uri, format) in srcs {
            let obj = js_sys::Object::new();
            set(&obj, "uri", &JsValue::from_str(&uri)).map_err(|e| JsError::new(&e))?;
            set(&obj, "format", &JsValue::from_str(&format)).map_err(|e| JsError::new(&e))?;
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Execute against the host-fetched `sources` (`[{ uri, format, bytes:
    /// Uint8Array }]`, `uri` and `format` as [`Self::sources`] emitted them) and
    /// return `{ files: [{ path, bytes: Uint8Array }], report }`.
    ///
    /// # Errors
    /// A JS `Error` carrying the message of any parse / staging / execution /
    /// encode failure.
    // The shared borrow is held across the run on purpose: it is what makes a
    // `registerDocument` issued mid-run fail instead of changing the program
    // under it.
    #[allow(clippy::await_holding_refcell_ref)]
    pub async fn run(&self, sources: JsValue, dest: String) -> Result<JsValue, JsError> {
        let sources = parse_sources(&sources).map_err(|e| JsError::new(&e))?;
        let exec = self.borrow()?;
        let out = exec
            .execute(sources, &dest)
            .await
            .map_err(|e| JsError::new(&e))?;
        out_to_js(&out).map_err(|e| JsError::new(&e))
    }
}

impl FossilExecutor {
    fn borrow(&self) -> Result<std::cell::Ref<'_, Executor>, JsError> {
        self.inner
            .try_borrow()
            .map_err(|_| JsError::new("the executor is being changed"))
    }

    fn borrow_mut(&self, call: &str) -> Result<std::cell::RefMut<'_, Executor>, JsError> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| JsError::new(&format!("{call} during a run")))
    }
}

/// Parse the JS `connections` object `{ name: baseUrl }`.
fn parse_connections(connections: &JsValue) -> Result<HashMap<String, String>, String> {
    let obj: &js_sys::Object = connections
        .dyn_ref::<js_sys::Object>()
        .ok_or("`connections` must be an object of { name: baseUrl }")?;
    let mut map = HashMap::new();
    for entry in js_sys::Object::entries(obj).iter() {
        let pair: js_sys::Array = entry.into();
        let name = pair
            .get(0)
            .as_string()
            .ok_or("`connections` keys must be strings")?;
        let url = pair
            .get(1)
            .as_string()
            .ok_or("`connections` values must be strings")?;
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
        let format = source_row(&get_string(&item, "format")?)?;
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

/// Marshal [`ExecOutput`] to `{ files: [{ path, bytes: Uint8Array }], report }`.
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
    let report = serde_wasm_bindgen::to_value(&out.report).map_err(|e| e.to_string())?;
    let result = js_sys::Object::new();
    set(&result, "files", &files)?;
    set(&result, "report", &report)?;
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
