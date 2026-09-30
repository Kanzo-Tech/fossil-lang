//! `fossil-df-wasm` — the `DataFusion` executor ([`fossil_df`]) exposed to JS.
//!
//! The browser runs the mapping. An [`Executor`] holds one compiled program and
//! is a document workspace: it reports the documents the program names and does
//! not hold ([`Executor::missing_documents`]), the host reads them through its
//! `Host` and registers each one, and only then are the sources listed
//! and the run executed — against the output descriptor decoded from those
//! registered documents, which is what the checker read. [`Executor::execute`]
//! builds a `DataFusion` plan (`lower_to_mir_pg` → `execute_graph`),
//! materialises the `GraphAr` graph, writes it under the destination, and
//! answers the [`RunReport`] — the manifest it wrote, as JSON. No mapping
//! runtime on the server.
//!
//! ## Storage seam
//! Every byte goes through a [`Storage`]: the sources `DataFusion` reads, the
//! RDF a provider decodes, and the files the run writes. A host with storage
//! builds it from the credentials its `Host` vends ([`FossilExecutor::run`]),
//! so `DataFusion` reads a remote source by range requests rather than whole; a
//! host with none holds the files in memory ([`FossilExecutor::run_in_memory`]).
//! Both are `object_store` stores, so the run cannot tell them apart.
//!
//! ## Native vs wasm split
//! The pure-Rust [`Executor`] (target-agnostic — `cargo test` drives it on a
//! tokio runtime) holds all the logic; the `#[wasm_bindgen]` [`FossilExecutor`]
//! wrapper only marshals JS values (and is driven by JS's event loop in the
//! browser, no tokio). Mirrors `fossil-wasm`'s `FossilWorkspace` split.

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
use fossil_base::{FossilDb, FsError, Provider, SourceFile, System};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_df::RunReport;
use fossil_df::SourceFormat;
use fossil_hir::documents::MissingDocument;
use fossil_layout::io::MemoryFs;
use fossil_storage::{Access, JsHost, Scope, Storage};
use futures::{StreamExt, TryStreamExt};
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};
use url::Url;
use wasm_bindgen::prelude::*;

pub mod session;

/// Files written at once.
const WRITES_IN_FLIGHT: usize = 4;

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

    /// The program's sources as `(locator, row-name, connection)`: what
    /// [`Self::execute`] reads. The second element is the catalogue row's name,
    /// which is what the program wrote after `io.`.
    ///
    /// # Errors
    /// The output shape document is unregistered or does not decode.
    pub fn sources(&self) -> Result<Vec<(String, String, Option<String>)>, String> {
        let descriptor = self.descriptor()?;
        Ok(
            fossil_df::program_sources(&self.db, self.file, &descriptor, &self.connections)
                .into_iter()
                .map(|s| (s.uri, format_kind(&s.format).to_owned(), s.connection))
                .collect(),
        )
    }

    fn descriptor(&self) -> Result<OutputDescriptorKind, String> {
        fossil_df::output_descriptor(&self.db, self.file)
    }

    /// Run the program on `DataFusion` over `storage` and write the `GraphAr`
    /// output under `dest`, a prefix `storage` covers: route each source —
    /// through its connection's credential, or as a public URL — register the
    /// stores, execute the graph, lay it out, and write it.
    ///
    /// # Errors
    /// A source or the destination no store covers, `DataFusion` execution,
    /// Parquet encode, or a write — each as a message for the JS host.
    pub async fn execute(&self, storage: &mut Storage, dest: &str) -> Result<RunReport, String> {
        let (db, file, connections) = (&self.db, self.file, &self.connections);
        let descriptor = self.descriptor()?;
        if !dest.ends_with('/') || !storage.covers(&format!("{dest}graph.graph.yml")) {
            return Err(format!("no store covers the destination {dest}"));
        }

        for source in fossil_df::program_sources(db, file, &descriptor, connections) {
            if storage.covers(&source.uri) {
                continue;
            }
            match source.connection {
                Some(connection) => storage
                    .grant(Scope::Connection(connection), Access::Read)
                    .await
                    .map(drop),
                None => storage.public(&source.uri),
            }
            .map_err(|e| format!("source {}: {e}", source.uri))?;
        }

        // No operator in any plan of this session spawns: the browser has no
        // Tokio runtime to spawn on. `session` says how, and what it cannot prove.
        let ctx = session::session();
        for (authority, store) in storage.stores() {
            let url = Url::parse(authority).map_err(|e| format!("{authority}: {e}"))?;
            ctx.register_object_store(&url, store);
        }
        register_rdf_sources(&ctx, db, file, &descriptor, storage, connections).await?;

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
        graph.declare_tiles(layout.tiles);
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
        let payload = fs.drain();
        let manifests = graph.manifest_files().map_err(|e| format!("encode: {e}"))?;
        write_all(storage, dest, payload.into_iter()).await?;
        write_all(
            storage,
            dest,
            manifests
                .into_iter()
                .map(|f| (f.rel_path, bytes::Bytes::from(f.bytes))),
        )
        .await?;
        Ok(report)
    }
}

/// Write each `(relative path, bytes)` under `dest`, a few at once.
async fn write_all(
    storage: &Storage,
    dest: &str,
    files: impl Iterator<Item = (String, bytes::Bytes)>,
) -> Result<(), String> {
    futures::stream::iter(files)
        .map(|(rel, bytes)| async move {
            let locator = format!("{dest}{rel}");
            storage
                .put(&locator, bytes)
                .await
                .map_err(|e| format!("write: {e}"))
        })
        .buffer_unordered(WRITES_IN_FLIGHT)
        .try_collect::<Vec<()>>()
        .await
        .map(drop)
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
            })
        })
        .collect();

    let report = fossil_layout::layout::enrich_layout_with(&fs, &targets, &adjacencies)
        .map_err(|e| format!("layout: {e}"))?;

    Ok((fs, report))
}

/// A source format as the wire names it — **the catalogue row's name**, which
/// is what a program wrote after `io.`.
///
/// The `Provider` arm carries the name rather than the literal `"rdf"` because
/// more than one row can be materialised: labelling an `io.avro` source `rdf`
/// would send it back as the RDF row. That `rdf` is the only materialised row
/// today is what would make the literal correct by coincidence.
fn format_kind(f: &SourceFormat) -> &str {
    match f {
        // `{ .. }` because `Csv` carries the `delimiter =` the program wrote.
        // It is not in the wire string on purpose: the delimiter is read by
        // `fossil_df::read_source` off the MIR the executor already holds.
        SourceFormat::Csv { .. } => "csv",
        SourceFormat::Json => "json",
        SourceFormat::Parquet => "parquet",
        SourceFormat::Provider { name } => name.as_str(),
    }
}

/// Decode + register every provider (RDF) source through
/// [`fossil_df::register_rdf`], its bytes read through `storage`.
///
/// It is the browser counterpart of `fossil_df::register_provider_sources`,
/// which walks the same `provider_bindings` and calls the same `register_rdf`;
/// where the bytes come from is the whole difference.
///
/// **"Exactly the counterpart" is a reading, not a result.** Nothing here
/// drives an RDF source through both halves and compares — `tests/` is CSV — so
/// the two can drift and only a program would notice.
async fn register_rdf_sources(
    ctx: &SessionContext,
    db: &dyn fossil_base::Db,
    file: SourceFile,
    descriptor: &OutputDescriptorKind,
    storage: &Storage,
    connections: &HashMap<String, String>,
) -> Result<(), String> {
    for binding in fossil_df::provider_bindings(db, file, descriptor, connections) {
        let bytes = storage
            .get(&binding.uri)
            .await
            .map_err(|e| format!("RDF source: {e}"))?;
        let turtle = std::str::from_utf8(&bytes)
            .map_err(|e| format!("RDF source `{}` is not UTF-8: {e}", binding.uri))?;
        fossil_df::register_rdf(ctx, &binding, turtle)
            .map_err(|e| format!("register RDF `{}`: {e}", binding.uri))?;
    }
    Ok(())
}

// ───────────────────────── wasm-bindgen surface ─────────────────────────────

/// The JS-facing executor — `FossilExecutor` on the JS side, a `RefCell`
/// around the Rust [`Executor`], for the reason `fossil-wasm`'s
/// `WasmWorkspace` gives: a `&mut self` export borrows wasm-bindgen's cell
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
    /// `@fossil-lang/storage`'s `resolveDocuments` sets it from the host.
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

    /// `[{ key, locator, connection? }]` — the documents the program names and
    /// the executor does not hold. `@fossil-lang/storage`'s `resolveDocuments`
    /// reads them.
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
            if let Some(connection) = &missing.connection {
                set(&obj, "connection", &JsValue::from_str(connection))
                    .map_err(|e| JsError::new(&e))?;
            }
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

    /// `[{ uri, format, connection? }]` — the sources to read, `uri` being the
    /// locator fossil resolved, `format` the catalogue row's name and
    /// `connection` the one a credential is vended for.
    ///
    /// # Errors
    /// A JS `Error` if the output shape document is unregistered or does not
    /// decode.
    pub fn sources(&self) -> Result<JsValue, JsError> {
        let srcs = self.borrow()?.sources().map_err(|e| JsError::new(&e))?;
        let arr = js_sys::Array::new();
        for (uri, format, connection) in srcs {
            let obj = js_sys::Object::new();
            set(&obj, "uri", &JsValue::from_str(&uri)).map_err(|e| JsError::new(&e))?;
            set(&obj, "format", &JsValue::from_str(&format)).map_err(|e| JsError::new(&e))?;
            if let Some(connection) = &connection {
                set(&obj, "connection", &JsValue::from_str(connection))
                    .map_err(|e| JsError::new(&e))?;
            }
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Run with the storage `host` vends: read each source through its
    /// connection's credential, write under the one prefix `host` vends `write`
    /// on for `job`, and answer the [`RunReport`].
    ///
    /// # Errors
    /// A JS `Error` when the host vends no single prefix to write under, or the
    /// run fails.
    // The shared borrow is held across the run on purpose: it is what makes a
    // `registerDocument` issued mid-run fail instead of changing the program
    // under it.
    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    pub async fn run(&self, host: JsValue, job: String) -> Result<JsValue, JsError> {
        let mut storage = Storage::new(Arc::new(JsHost::new(host)));
        let scope = Scope::Job(job);
        let dest = match storage.grant(scope.clone(), Access::Write).await {
            Ok([prefix]) => prefix.clone(),
            Ok(prefixes) => {
                return Err(JsError::new(&format!(
                    "the host vended {} write credentials for {scope}; a run writes under exactly one prefix",
                    prefixes.len()
                )));
            }
            Err(e) => return Err(JsError::new(&e.to_string())),
        };
        let exec = self.borrow()?;
        let report = exec
            .execute(&mut storage, &dest)
            .await
            .map_err(|e| JsError::new(&e))?;
        Ok(serde_wasm_bindgen::to_value(&report)?)
    }

    /// Run over files held in memory, for a host with no storage: `sources`
    /// maps each locator the program reads to its bytes, and the output stays
    /// in memory under `dest`. Answers `{ files: [{ path, bytes }], report }`.
    ///
    /// # Errors
    /// A JS `Error` when `sources` is not an object of `Uint8Array`s, or the run
    /// fails.
    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    #[wasm_bindgen(js_name = runInMemory)]
    pub async fn run_in_memory(&self, sources: JsValue, dest: String) -> Result<JsValue, JsError> {
        let dest = format!("{}/", dest.trim_end_matches('/'));
        let mut storage = Storage::new(Arc::new(NoHost));
        let mut held: HashMap<String, Arc<InMemory>> = HashMap::new();
        let mut memory = |storage: &mut Storage, root: &str| -> Result<Arc<InMemory>, JsError> {
            if let Some(store) = held.get(root) {
                return Ok(Arc::clone(store));
            }
            let store = Arc::new(InMemory::new());
            storage
                .with_store(root, Arc::clone(&store) as Arc<dyn ObjectStore>)
                .map_err(|e| JsError::new(&e.to_string()))?;
            held.insert(root.to_string(), Arc::clone(&store));
            Ok(store)
        };
        let obj: &js_sys::Object = sources
            .dyn_ref()
            .ok_or_else(|| JsError::new("`sources` is `{ [locator]: Uint8Array }`"))?;
        for entry in js_sys::Object::entries(obj).iter() {
            let pair: js_sys::Array = entry.into();
            let locator = pair
                .get(0)
                .as_string()
                .ok_or_else(|| JsError::new("a source locator is a string"))?;
            let bytes: js_sys::Uint8Array = pair
                .get(1)
                .dyn_into()
                .map_err(|_| JsError::new("a source's bytes are a Uint8Array"))?;
            let url = Url::parse(&locator).map_err(|e| JsError::new(&format!("{locator}: {e}")))?;
            let root = format!("{}://{}/", url.scheme(), url.authority());
            let store = memory(&mut storage, &root)?;
            store
                .put(
                    &object_store::path::Path::from(url.path().trim_start_matches('/')),
                    bytes.to_vec().into(),
                )
                .await
                .map_err(|e| JsError::new(&e.to_string()))?;
        }
        let out = Arc::new(InMemory::new());
        storage
            .with_store(&dest, Arc::clone(&out) as Arc<dyn ObjectStore>)
            .map_err(|e| JsError::new(&e.to_string()))?;

        let exec = self.borrow()?;
        let report = exec
            .execute(&mut storage, &dest)
            .await
            .map_err(|e| JsError::new(&e))?;

        let key = Url::parse(&dest)
            .map_err(|e| JsError::new(&e.to_string()))?
            .path()
            .trim_start_matches('/')
            .to_string();
        let files = js_sys::Array::new();
        let listed: Vec<_> = out
            .list(None)
            .try_collect()
            .await
            .map_err(|e| JsError::new(&e.to_string()))?;
        for meta in listed {
            let bytes = out
                .get(&meta.location)
                .await
                .map_err(|e| JsError::new(&e.to_string()))?
                .bytes()
                .await
                .map_err(|e| JsError::new(&e.to_string()))?;
            let location = meta.location.as_ref();
            let path = location.strip_prefix(&key).unwrap_or(location);
            let file = js_sys::Object::new();
            set(&file, "path", &JsValue::from_str(path)).map_err(|e| JsError::new(&e))?;
            set(&file, "bytes", &js_sys::Uint8Array::from(bytes.as_ref()))
                .map_err(|e| JsError::new(&e))?;
            files.push(&file);
        }
        let result = js_sys::Object::new();
        set(&result, "files", &files).map_err(|e| JsError::new(&e))?;
        set(&result, "report", &serde_wasm_bindgen::to_value(&report)?)
            .map_err(|e| JsError::new(&e))?;
        Ok(result.into())
    }
}

/// The host of a run with no storage: it has no connections and vends nothing,
/// so a source that is not in memory is refused by name.
#[derive(Debug)]
struct NoHost;

impl fossil_storage::Host for NoHost {
    fn connections(
        &self,
    ) -> futures::future::BoxFuture<'static, Result<HashMap<String, String>, String>> {
        Box::pin(async { Ok(HashMap::new()) })
    }

    fn credentials(
        &self,
        scope: &Scope,
        _access: Access,
    ) -> futures::future::BoxFuture<'static, Result<Vec<fossil_storage::StorageCredential>, String>>
    {
        let refused = format!("a run in memory has no storage to vend {scope} from");
        Box::pin(async move { Err(refused) })
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

fn set(obj: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), String> {
    js_sys::Reflect::set(obj, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|e| js_err(&e))
}

fn js_err(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| "JS error".to_string())
}
