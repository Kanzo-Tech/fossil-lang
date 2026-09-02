//! `fossil-df-wasm` — the `DataFusion` executor ([`fossil_df`]) exposed to JS.
//!
//! The browser runs the mapping: keasy (the thin server) hands the playground a
//! fossil program + signed source URLs; JS `fetch`es each source's bytes and
//! calls [`FossilExecutor::run`], which builds a `DataFusion` plan
//! (`lower_to_mir_pg` → `execute_graph`), materialises the `GraphAr` graph, and
//! returns the output files (Parquet + manifest YAML, as bytes) plus the
//! [`RunReport`] — the same manifest, as JSON, so the host does not have to
//! parse back out of the bytes it is about to upload. JS then signed-`PUT`s the
//! files and `PATCH`es the job. No mapping runtime on the server.
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
use fossil_base::{FossilDb, FsError, Provider, RowReader, SourceFile, System};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_df::RunReport;
use fossil_df::SourceFormat;
use fossil_df::files::GraphArFile;
use fossil_shex::ShExDescriptor;
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
/// the object-store / provider seams, never through `System::read_file`, and
/// touches no clock on its path — so `read_file` is unreachable (returns
/// `NotFound`) and `now` returns the wasm-safe `UNIX_EPOCH` placeholder
/// (`SystemTime::now()` panics on `wasm32-unknown-unknown`).
///
/// `descriptors` stays at its trait default `None`: typing falls back to the
/// passed output descriptor / string defaults — the descriptor is an argument,
/// not read here.
///
/// **`providers` is no longer the default, and the argument for the default was
/// overtaken.** It used to be `fossil_base::providers::DATA` — the four rows
/// that read DATA and **no row that reads types** — on the reasoning that the
/// schema arrives already decided as the `shex` argument to [`execute_core`],
/// so a type-reading row would put a second, differently sourced answer to
/// "what shape does this program write?" inside the database.
///
/// That reasoning held while a mapping header carried its own shape IRI
/// (`Person : ex:Person from users`). Since ruling 3 of 2026-08-11 the header
/// names a BARE name bound positionally by `type { … } := io.shex("…")`, so the
/// shape IRI — and with it the vertex label and every property's `rdf_uri` —
/// comes from the registered DOCUMENT and from nowhere else. Without a decoder
/// row the executor wrote `vertex/.parquet` and columns whose `rdf_uri` was
/// `None`, and neither is a second answer: it is no answer. There is still one
/// schema here; [`build_program`] registers that one text under the name the
/// program writes, so both consumers read the same bytes.
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

/// Run a fossil program on `DataFusion` from host-fetched source bytes — the
/// target-agnostic core (`cargo test` drives it natively; the browser drives it
/// via the wasm wrapper). Builds the db + output descriptor, stages each source
/// in the [`SessionContext`], executes the graph, and encodes the `GraphAr` output.
///
/// `dest` is the dataset's logical location (e.g. the job's object-storage
/// prefix) — it only labels the [`RunReport`], no IO happens against it here.
///
/// # Errors
/// `ShEx` parse, URL parse, object-store staging, `DataFusion` execution, or
/// Parquet encode failures — all surfaced as a message string for the JS host.
// The hasher is not ours to choose: `connections` arrives from
// `parse_refs(&JsValue)`, which builds a plain `HashMap`. Generalising the
// signature over `BuildHasher` would add a parameter no caller can vary.
#[allow(clippy::implicit_hasher)]
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
    let report = RunReport::of(dest, &graph);
    let files = enrich_layout_in_memory(&graph, files)?;
    Ok(ExecOutput { files, report })
}

/// Run the real layout pass over the staged corpus, in memory.
///
/// This is `fossil_cli::host::enrich_written_layout` with a different
/// filesystem under it, and the two are deliberately the same shape: the same
/// targets, the same adjacencies in both orientations, the same deletions
/// afterwards. **What the tab writes has to be what `fossil run` writes**, and
/// the only way to be sure of that is for the browser to run the pass rather
/// than to approximate it.
///
/// It is not optional. `fossil-df` writes `x`, `y` and `cluster_id` as zeroed
/// placeholders and declares in the manifest the tree this pass delivers, so a
/// browser that skips it publishes a corpus satisfying the manifest's *shape*
/// and violating the property that shape exists to express — `dense_id`
/// ascending with the Morton code of the vertex's position
/// (`/docs/format/conventions/addressing`) — and every count-based check
/// passes.
///
/// What it costs: one resident copy of every file, twice over at the crossing
/// point, because there is no filesystem here to stream to as the native pass
/// does. The pass is the memory-hungry half of the write path
/// (`/docs/design/streaming`) and `wasm32`'s address space is 4 GiB, so a
/// corpus that fits natively can fail here. That ceiling is a property of the
/// target, stated rather than worked around.
fn enrich_layout_in_memory(
    graph: &fossil_df::GraphArData,
    files: Vec<GraphArFile>,
) -> Result<Vec<GraphArFile>, String> {
    use fossil_layout::io::MemoryFs;
    use fossil_layout::layout::{AdjacencyTarget, Endpoint, VertexLayoutTarget};

    // The keys are the corpus-relative paths the files already carry, so a
    // target composed below names the same string the executor emitted. No
    // `dest` prefix: the native host joins one because it writes into a
    // directory, and this writes into a map whose keys are what JS receives.
    let fs = MemoryFs::new();
    for file in files {
        fs.insert(file.rel_path, file.bytes);
    }

    let adjacency = |e: &fossil_df::EdgeTable, file: &str| {
        format!(
            "edge/{}_{}_{}/{file}.parquet",
            e.src_type, e.label, e.dst_type
        )
    };

    let targets: Vec<VertexLayoutTarget> = graph
        .schema
        .nodes
        .iter()
        .map(|node| VertexLayoutTarget {
            type_name: node.label.clone(),
            vertex_parquet: format!("vertex/{}.parquet", node.label),
            // Trailing separator: the layout appends the tiles file.
            chunk_prefix: format!("vertex/{}/", node.label),
            // The same constant the manifest is written with, so the files and
            // the promise cannot drift apart.
            chunk_size: fossil_sinks::manifest::DEFAULT_CHUNK_SIZE,
            self_edge_csr: graph
                .edges
                .iter()
                .filter(|e| e.src_type == node.label && e.dst_type == node.label)
                .map(|e| adjacency(e, "by_source"))
                .collect(),
        })
        .collect();

    // Every adjacency file, both orientations, cross-type included. The pass
    // renumbers `dense_id`, so a file left out keeps ids that now belong to
    // somebody else — a silent corruption, which is why this enumerates rather
    // than letting the layout guess.
    let adjacencies: Vec<AdjacencyTarget> = graph
        .edges
        .iter()
        .flat_map(|e| {
            [
                (adjacency(e, "by_source"), Endpoint::Src),
                (adjacency(e, "by_target"), Endpoint::Dst),
            ]
            .map(|(parquet, ordered_by)| AdjacencyTarget {
                parquet,
                src_type: e.src_type.clone(),
                dst_type: e.dst_type.clone(),
                ordered_by,
            })
        })
        .collect();

    fossil_layout::layout::enrich_layout_with(&fs, &targets, &adjacencies)
        .map_err(|e| format!("layout: {e}"))?;

    // The staged single-file vertex Parquet was this pass's input and nothing
    // reads it afterwards — the manifest points at the chunk prefix. Left in, it
    // is a second, stale copy of every vertex, and `apps/corpus`'s
    // `exactly-once` fails a corpus for exactly that.
    for target in &targets {
        fs.remove(&target.vertex_parquet);
    }
    // And the staged adjacencies: the pass read each one pre-renumbering, so
    // leaving it publishes the relation with ids that now belong to other
    // vertices — and publishes it beside its own tiles, which is two containers
    // for one set of rows (`declared-tiling`).
    for adjacency in &adjacencies {
        fs.remove(&adjacency.parquet);
    }

    Ok(fs
        .drain()
        .into_iter()
        .map(|(rel_path, bytes)| GraphArFile {
            rel_path,
            bytes: bytes.to_vec(),
        })
        .collect())
}

/// Enumerate the program's sources as `(uri, row-name)` — the target-agnostic
/// core behind [`FossilExecutor::sources`]. Pure (no IO); the host uses it to
/// plan its fetches before [`execute_core`]. The second element is the catalogue
/// row's name, which is what the program wrote after `io.` and what the host
/// hands back on `SourceInput.format`.
///
/// # Errors
/// `ShEx` parse failures.
#[allow(clippy::implicit_hasher)] // same as `execute_core` above
pub fn program_sources_core(
    program: &str,
    shex: Option<&str>,
    connections: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    let (db, file, descriptor) = build_program(program, shex)?;
    Ok(
        fossil_df::program_sources(&db, file, &descriptor, connections)
            .into_iter()
            .map(|s| (s.uri, format_kind(&s.format).to_owned()))
            .collect(),
    )
}

/// Build the executor's db + interned program + output descriptor — shared by
/// [`execute_core`] and [`program_sources_core`].
///
/// **The one schema this host holds is registered under the name the program
/// writes, and is also the descriptor.** It used to be registered nowhere: the
/// other hosts register what `type { … } := io.shex("…")` names because their
/// checker has to decode it, and this one installed no decoder row, so a
/// registered document could only have sat in the database unread.
///
/// A bare property key means the last segment of a predicate IRI the DOCUMENT
/// declares, and a bare header name is bound positionally against the same
/// document — so with nothing registered the mapping still compiled (an
/// unregistered document is informational, not fatal) and produced an empty
/// predicate table and an empty shape IRI. Measured: `vertex/.parquet`, and a
/// `VertexInfo` whose `iri` was the empty string where the shape's type IRI
/// belongs.
///
/// The order is forced and it is the native host's: parse → ask the def-map what
/// the program names → register → compile. Registering bumps the registry's
/// revision, so the `def_map` computed here is re-derived once on the way to the
/// plan. This host reads no filesystem, so there is exactly one text to
/// register and no loop: whatever the first `type` binding names is what the
/// browser fetched.
fn build_program(
    program: &str,
    shex: Option<&str>,
) -> Result<(FossilDb, SourceFile, OutputDescriptorKind), String> {
    let system: Arc<dyn System> = Arc::new(ExecutorSystem);
    let mut db = FossilDb::new(system);
    let file = SourceFile::new(&db, program.to_string(), "program.fossil".to_string());

    // The key must be what the reader passes to `file_at` — the path the
    // program wrote, resolved against the program's own directory. It is
    // `fossil_hir::documents::registry_key`, the same function the checker
    // looks the document up with and the other two hosts register under, so
    // there is nothing to drift: a key that stops matching reads exactly like a
    // document nobody registered.
    if let Some(text) = shex
        && let Some(document) = fossil_hir::def_map::def_map(&db, file).output_shape_document(&db)
    {
        let key = fossil_hir::documents::registry_key(&db, file, &document);
        let doc = SourceFile::new(&db, text.to_string(), key.clone());
        fossil_base::register_file(&mut db, key, doc);
    }

    // AFTER the registration, not before: a `@rename` is addressed to a `type`
    // binding's NAME, and that name is bound positionally against the decoded
    // document — so the shape IRI the rename is keyed by does not exist until
    // the document is in the database.
    let descriptor = match shex {
        Some(text) => {
            build_descriptor(text, &fossil_hir::def_map::def_map(&db, file).renames(&db))?
        }
        None => OutputDescriptorKind::ACCEPT_ALL_DEFAULT,
    };

    Ok((db, file, descriptor))
}

/// Build the output descriptor from a schema blob, auto-detecting its language.
/// The host passes one opaque `schema` string; we route it to the right lowering:
/// `ShExJ` (JSON, leading `{`) and `ShExC` go through `fossil-shex`; a SHACL
/// shapes graph (Turtle carrying the SHACL namespace / `sh:NodeShape` /
/// `sh:property`) is walked into a `GraphSchema`. All three end as the canonical
/// model the executor consumes via `OutputDescriptorKind::to_graph_schema`.
fn build_descriptor(
    text: &str,
    renames: &fossil_graph_schema::Renames,
) -> Result<OutputDescriptorKind, String> {
    let is_json = text.trim_start().starts_with('{');
    let looks_shacl = !is_json
        && (text.contains("http://www.w3.org/ns/shacl#")
            || text.contains("sh:NodeShape")
            || text.contains("sh:property"));
    if looks_shacl {
        // Through the SHACL ROW, which is the one SHACL lowering there is now —
        // it used to be a second implementation in `fossil-df` producing a
        // `GraphSchema` directly, one step past the vocabulary the checker
        // reads, which is why SHACL reached the executor and never the checker.
        return Ok(OutputDescriptorKind::Lowered(
            fossil_descriptors_output::decode_shacl("", text)
                .map_err(|e| format!("SHACL parse error: {e:?}"))?
                .to_graph_schema(renames),
        ));
    }
    Ok(OutputDescriptorKind::ShEx(
        ShExDescriptor::from_shex_source(text).map_err(|e| format!("ShEx parse error: {e:?}"))?,
    ))
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
        SourceFormat::Csv => "csv",
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
    // `wasm_bindgen` dictates these by value: it owns the JS→Rust conversion
    // and cannot hand out borrows across the boundary.
    #[allow(clippy::needless_pass_by_value)]
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
            set(&obj, "format", &JsValue::from_str(&format)).map_err(|e| JsError::new(&e))?;
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Execute `program` against the host-fetched `sources` and return
    /// `{ files: [{ path, bytes: Uint8Array }], report }`.
    ///
    /// `sources` is a JS array of `{ uri, format, bytes: Uint8Array }` (the `uri`
    /// being the RESOLVED one [`Self::sources`] returned); `format` is the
    /// catalogue row's name — what the program wrote after `io.`, and exactly the
    /// string [`Self::sources`] emitted. `shex` is the optional output schema
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
