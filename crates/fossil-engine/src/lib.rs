//! **`fossil-engine` is fossil's native host.** It supplies the [`fossil_base::System`] the
//! compiler runs against, does the pre-compile jobs the compiler cannot do for
//! itself — introspect the sources, register the shape documents a program
//! names, install the cloud secrets a `@conn` needs — and drives the
//! compile→run pipeline, returning structured data the binaries render.
//!
//! `fossil-wasm` is the same shape for the browser and is worth reading beside
//! this: a `System` over an in-memory file map, `registerInferredDescriptor`
//! where this crate has `pre_introspect_and_register`, and the same one-line
//! projections of `fossil_lineage::{providers, source_refs}`. Where the two
//! differ is the outside world — a disk, a `DuckDB`, a credential — which is
//! `/docs/design/three-hosts`' cut, and it is the whole of the difference.
//!
//! # What is not a host job, and is still here
//!
//! [`census`] is a compiler query behind a native tripwire, reachable only from
//! `tests/programs.rs`. It reads the CST against the `HirBody` it produced and
//! belongs in `fossil-hir` or beside the harness; it is here because it started
//! here. `crates/fossil-engine/src/census.rs` says which, and why the module
//! doc's own premise is now stale.
//!
//! [`run`]'s layout post-pass re-derives `fossil-df`'s on-disk path convention
//! (`vertex/<T>.parquet`, `vertex/<T>/`, `edge/<s>_<l>_<d>/by_{source,target}`)
//! in order to find files `fossil-df` wrote and delete them. That is behavioural
//! coupling with no compiler-visible signature, and it has already shipped one
//! defect — see [`enrich_written_layout`].
//!
//! # What left, and why
//!
//! The `Diagnostic` drain — «what is wrong with this program» — is not here any
//! more. It was one capability answered three ways (this crate, `fossil-lsp`,
//! `fossil-wasm`) and the two editor answers were the short one, missing three
//! classes of diagnostic outright. It is [`fossil_mir::program_diagnostics`],
//! which all three hosts now call.

#![allow(rustdoc::private_intra_doc_links)] // `enrich_written_layout` is named
// above because it is where the coupling lives, and a reader who follows the
// pointer should land on the code rather than on a paraphrase of it.

// **The reason changed, and that is the point of the change.** This said
// «depends on fossil-layout which uses bundled DuckDB», then «its own bundled
// DuckDB, for `pre_introspect`». Neither is true: introspection and credentials
// are `fossil-introspect`'s, and this crate links no database.
//
// What is left is `run`, and it is the FILESYSTEM. `fossil_df::run_to_dir` is
// itself `cfg(not(wasm32))` because it writes a GraphAr tree to a local
// directory, which a browser has no concept of — measured by building for the
// target, and it is the only remaining error. That does not contradict «fossil
// is a compiler consumed as a WASM library»: writing files to a disk is what a
// native host DOES, and the browser's host writes bytes back over its own seam.
//
// So `check` is wasm-capable and `run` is not, in one crate. Splitting them is a
// decision and not a cleanup; `docs/design/one-engine.mdx` records it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fossil_base::{Diagnostic, SourceAnchor};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_df::RunReport;
use fossil_lineage::{ProviderInfo, SourceRefInfo};
use smol_str::SmolStr;

pub mod census;
mod documents;
mod system;

pub use system::host_system;

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-engine is native-only: `run` writes a GraphAr tree to a local \
     directory through `fossil_df::run_to_dir`, which is itself wasm-gated"
);

use system::open_db;

// ===================================================================== providers

/// List the data-source providers fossil supports. Thin native wrapper over
/// [`fossil_lineage::providers`] (the shared, WASM-clean implementation — one
/// source of truth for both the CLI and the browser).
#[must_use]
pub fn providers() -> Vec<ProviderInfo> {
    // The engine's own table, which is the whole registry — so `fossil
    // providers` now lists `shex` and `shacl` as `Schema` rows beside the four
    // `Data` ones. The wire contract has carried that distinction since it was
    // written and nothing ever produced anything but `Data`.
    fossil_lineage::providers(fossil_descriptors_output::PROVIDERS)
}

// ========================================================================= refs

/// Parse a program and return its external references (data URI + `schema =` for
/// every source), each tagged with the `@conn` alias it targets (or `None` for a
/// direct URL/path). Parse-only — the typed lineage keasy reads to derive a job's
/// connection set without scanning script text.
///
/// # Errors
/// Returns a read error if `path` is unreadable.
pub fn refs(path: &Path) -> miette::Result<Vec<SourceRefInfo>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let (db, file) = open_db(text, path);
    // The native host reads the file; the lineage logic (parse → typed refs,
    // dedup) is the shared WASM-clean `fossil_lineage::source_refs` — same code
    // the browser runs over its in-memory db.
    Ok(fossil_lineage::source_refs(&db, file))
}

// ======================================================================== check

/// The result of [`check`]: the source text (for rendering spans) + every
/// accumulated diagnostic. The caller decides how to render and whether to fail
/// (exit non-zero iff any [`fossil_base::Severity::Error`] is present).
#[derive(Debug)]
pub struct CheckOutcome {
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
    /// The shape documents the program names, under the program's spelling,
    /// with their text — see [`crate::documents::named_document_texts`].
    ///
    /// A renderer needs them because a diagnostic can point INTO one
    /// (`fossil_base::SpanLabel::document`) and miette resolves a range against
    /// one source at a time. Empty for a program that names none, and for one
    /// whose documents could not be read — which is the same thing the checker
    /// saw.
    pub documents: Vec<(String, String)>,
    /// How many mappings the file defines. Zero with no diagnostics is a
    /// program that parses and builds nothing — the caller renders that
    /// differently from a clean program, because it is not the same answer.
    pub mappings: usize,
}

/// The host half of `check`: read the file, stand up the native database, do
/// the host's pre-compile jobs, and ask
/// [`fossil_mir::program_diagnostics`] what is wrong with the program.
///
/// **Introspection is the caller's, and doing it is what makes the two commands
/// agree.** `check` and [`run`] both read whatever
/// `fossil_introspect::pre_introspect_and_register` put in the `System`'s
/// descriptor cache; neither fills it. A caller that skips it gets a `check`
/// with no forward-propagated source types — the same answer the browser gets
/// before `registerInferredDescriptor` has run, which is the shape this follows.
///
/// **The drain itself is not here, and used to be.** It is one capability
/// answered three ways — this crate, `fossil-lsp` and `fossil-wasm` — and the
/// two editor copies were the short version, missing the three classes of
/// diagnostic named in `fossil_mir::diagnostics`. What is left in this function
/// is the part only a native host can do.
///
/// Zero mappings and zero diagnostics is reported as success, not as an error:
/// `check` answers "is this text a well-formed program", and a program that
/// declares nothing is vacuously well-formed. `run` refuses it (`no mapping
/// found in …`) because `run` was asked to produce a graph and there is nothing
/// to produce it from — a different question, asked of a different command. So
/// the count rides out on [`CheckOutcome::mappings`] and the caller says so
/// plainly rather than claiming a clean bill of health it did not earn.
///
/// # Errors
/// Returns a read error if `path` is unreadable.
pub fn check(path: &Path) -> miette::Result<CheckOutcome> {
    tracing::debug!(?path, "fossil check");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let (db, file) = open_db(text.clone(), path);
    // `check` built a `SourceAnchor` here and used it for exactly one thing: the
    // pre-introspection. That went to the caller, and so did the anchor — which
    // is where the note it carried belongs too. `check` has no `--creds-stdin`,
    // so a caller resolves it with no connection map, and against the SAME
    // directory `run` will: the two used to differ, `check` introspecting
    // against `path.parent()` while the executor read against the process's cwd.
    let mappings = fossil_hir::def_map::def_map(&db, file).mappings(&db).len();
    Ok(CheckOutcome {
        source: text,
        diagnostics: fossil_mir::program_diagnostics(&db, file),
        mappings,
        documents: crate::documents::named_document_texts(&db, file),
    })
}

// The pre-introspection block stood here — `pre_introspect_and_register`, the
// DuckDB type table, the source scrape, the freshness token — and it is
// `fossil-introspect` now. It is what a HOST does before compiling, which the
// browser has always done from outside: `fossil-wasm` implements
// `System::descriptors` and `@fossil-lang/introspect` fills it. Doing it from
// inside this crate is what made the compiler open a DuckDB connection.
// ================================================================== run pipeline

/// Resolve the program-resident OUTPUT descriptor. The shape is sourced from the
/// PROGRAM, never a host flag (invariant #1).
///
/// # Two places a program can name its output shape, and the order between them
///
/// 1. **`type { … } := io.shex("shop.shex")`** — naming a shape document is
///    MANDATORY, so this binding always exists. It is what the CHECKER reads
///    ([`fossil_hir::def_map::DefMap::output_shape_document`]), and it is
///    consulted FIRST: the document the run classifies edges with has to be the
///    document the program compiled against, or a CSV program gets
///    `ACCEPT_ALL_DEFAULT` and emits zero edges.
/// 2. `io.rdf(schema = …)` — an RDF *input* whose `ShEx` doubles as the output
///    contract. It stays as the fallback for a program that reads a graph and
///    writes one back.
///
/// v1: one shape per program (a second, different `io.rdf` schema is rejected,
/// not merged).
fn resolve_output_descriptor(
    db: &fossil_base::FossilDb,
    def_map: fossil_hir::def_map::DefMap<'_>,
    anchor: SourceAnchor<'_>,
) -> miette::Result<OutputDescriptorKind> {
    // The `type { … } := io.shex(…)` binding, and it wins: it is the one the
    // checker resolved the mapping's target shape against, so preferring it is
    // what keeps «what compiled» and «what ran» the same document. The
    // CONSTRUCTOR travels with it, because it is what selects the row that
    // reads it, and reading a document with a row the program did not name is
    // how the run comes to use a different parser from the check.
    // The program's `@rename`s travel with the document, because they decide
    // the emitted column's name. Read off the same `def_map` — and read HERE
    // rather than inside the decode, so the one place that has the program is
    // the one place that supplies them.
    let renames = def_map.renames(db);

    if let Some((constructor, document)) = def_map.output_shape_binding(db) {
        return read_output_shape(constructor.as_deref(), document.as_str(), anchor, &renames);
    }

    // The `schema =` argument carries its OWN provider now
    // (`schema = io.shex("x.shex")`), so the pair travels together here exactly
    // as the `type { … }` pair does above — there is no position left where a
    // document arrives without the row that reads it.
    let mut schema: Option<(Option<SmolStr>, SmolStr)> = None;
    for s in def_map.sources(db) {
        let is_provider = s
            .constructor
            .as_deref()
            .and_then(|c| fossil_base::provider(fossil_descriptors_output::PROVIDERS, c))
            .is_some_and(|p| p.reads_rows == Some(fossil_base::RowReader::Materialised));
        if !is_provider {
            continue;
        }
        let Some(arg) = s.schema_arg.as_ref() else {
            continue;
        };
        match &schema {
            Some((_, existing)) if existing != arg => {
                return Err(miette::miette!(
                    "a program may declare only one io.rdf output shape (v1); found `{existing}` and `{arg}`"
                ));
            }
            _ => schema = Some((s.schema_provider.clone(), arg.clone())),
        }
    }

    let Some((provider, schema)) = schema else {
        return Ok(OutputDescriptorKind::ACCEPT_ALL_DEFAULT);
    };

    read_output_shape(provider.as_deref(), schema.as_str(), anchor, &renames)
}

/// Read and decode one shape document into the run's output descriptor,
/// **through the registry row the program named** — the row is selected by the
/// constructor the program wrote and its `reads_types` is the same `fn` the
/// checker calls, so «what compiled» and «what ran» are one decode of one set of
/// bytes by construction. Parsing the document any other way here is how a
/// document comes to type-check and then fail the run.
///
/// The descriptor is [`OutputDescriptorKind::Lowered`] whatever the language: it
/// means "the decode already happened". The rich `ShEx` resolved table it
/// replaces was only ever read by the checker, which does not come through here;
/// the executor reads `to_graph_schema` and nothing else.
fn read_output_shape(
    constructor: Option<&str>,
    document: &str,
    anchor: SourceAnchor<'_>,
    renames: &fossil_graph_schema::Renames,
) -> miette::Result<OutputDescriptorKind> {
    use fossil_base::providers::{Capability, provider};

    let table = fossil_descriptors_output::PROVIDERS;
    let ctor = constructor.ok_or_else(|| {
        miette::miette!(
            "the shape document `{document}` is named by no provider — write \
             `io.shex(\"…\")` or `io.shacl(\"…\")`"
        )
    })?;
    let row = provider(table, ctor).ok_or_else(|| {
        miette::miette!("{}", fossil_hir::refusals::unknown_constructor(ctor, table))
    })?;
    if !row.provides(Capability::ReadTypes) {
        return Err(miette::miette!(
            "{}",
            fossil_hir::refusals::decline_capability(row, Capability::ReadTypes, table)
        ));
    }
    if !row.accepts(document) {
        return Err(miette::miette!(
            "{}",
            fossil_hir::refusals::decline_extension(row, document)
        ));
    }
    let decode = row
        .reads_types
        .ok_or_else(|| miette::miette!("`{}` reads no types", row.constructor()))?;

    // The one resolution rule, and the same one the CHECKER went through to
    // read this document (`fossil_hir::def_map::resolve_relative`). A run that
    // anchored differently would decode a different file from the one that
    // type-checked, which is the same class of bug as decoding it with a
    // different parser.
    let locator = anchor.locator(document);
    let text = std::fs::read_to_string(&locator)
        .map_err(|e| miette::miette!("read output shape document `{locator}`: {e}"))?;
    let shapes = decode(&locator, &text)
        .map_err(|e| miette::miette!("parse output shape document `{locator}`: {e:?}"))?;
    Ok(OutputDescriptorKind::Lowered(
        shapes.to_graph_schema(renames),
    ))
}

// `connection_urls` and `apply_source_creds` went with it, and `mod creds` with
// them. This crate takes a `HashMap<String, String>` of connection URLs and
// never sees a secret — which is what lets it drop `fossil-resolver`, whose own
// wasm32 tripwire says cloud credentials must not cross that boundary.
/// The local filesystem directory a dest URL writes under, or `None` for a cloud
/// object store (which needs no directory pre-creation).
fn local_dest_dir(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(PathBuf::from(rest));
    }
    if url.contains("://") {
        return None; // cloud scheme — flat namespace, no mkdir
    }
    Some(PathBuf::from(url))
}

/// Compile + execute a `.fossil` file, materialising `GraphAr` `Parquet` to
/// `dest_url`. Returns the [`RunReport`] — the manifest it wrote, plus `dest`
/// and the edges the endpoint join discarded.
/// The output descriptor is program-resident (invariant #1). `creds` carries the
/// cloud config (empty ⇒ local / public-URL behaviour).
///
/// `memory_bytes` is the run's declared memory budget, and it is one number for
/// the whole run: the `DataFusion` pool the write path executes under and the
/// `DuckDB` `memory_limit` of the layout pass that follows it. Two engines spend
/// memory here; a budget that governed only one of them would be a budget for
/// half the run. `None` runs both unbounded.
///
/// # Errors
/// Returns a compile, read, or materialisation error.
#[allow(clippy::implicit_hasher)] // the host builds one connection map and hands
// it over; a generic hasher would be a parameter no caller varies.
pub fn run(
    path: &Path,
    dest_url: &str,
    connections: &HashMap<String, String>,
    memory_bytes: Option<u64>,
) -> miette::Result<RunReport> {
    tracing::debug!(?path, dest_url, "fossil run");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    // `run` does not read the text again: the pre-introspection that did — and
    // that is why this was a clone — is the caller's now.
    let (db, file) = open_db(text, path);
    // The run's one anchor: the directory of the program being run, and the
    // `@conn` map it expands aliases through. Everything below resolves through
    // this and nothing below consults the process's working directory — which
    // is what makes `fossil run apps/docs/programs/hello/hello.fossil` mean the
    // same thing from the repository root as from beside the program.
    let program_dir = fossil_base::program_dir(&path.to_string_lossy());
    let anchor = SourceAnchor::new(&program_dir, connections);
    // The CSV type pre-introspection that fed the type-checker's `source_row`
    // stood here. It is the CALLER's now — `fossil_introspect::pre_introspect_and_register`
    // against this db's `System`, before this call — which is the order the
    // browser has always used, and the reason `connections` arrives as URLs
    // rather than as credentials.

    let def_map = fossil_hir::def_map::def_map(&db, file);
    if def_map.mappings(&db).is_empty() {
        return Err(miette::miette!("no mapping found in {}", path.display()));
    }

    // The program-resident output descriptor (ShEx) drives the PG edge/cardinality
    // classification inside `execute_graph` (via `apply_output_shape`).
    let descriptor = resolve_output_descriptor(&db, def_map, anchor)?;

    // The single execution path: lower to the property-graph MIR + execute on
    // DataFusion + write the GraphAr tree. The host's only job is the byte seam
    // for provider (RDF) sources — resolve the URI (`@conn` + relative) and read
    // it; object-store formats stream through the executor's filesystem store.
    let dest_dir = local_dest_dir(dest_url).ok_or_else(|| {
        miette::miette!("the DataFusion run path writes a local directory; cloud dest `{dest_url}` is not yet wired")
    })?;
    // And the same refusal for a cloud SOURCE, which had none — measured against
    // a real MinIO on 2026-08-23 (`tests/cloud_source.rs`). A program reading
    // `@conn/users.csv` INTROSPECTS fine: the host's DuckDB installs the
    // connection's `CREATE SECRET` and the DESCRIBE comes back with real column
    // types, so the program type-checks against the columns it actually has.
    // Then the run reached DataFusion, which has no object store registered for
    // the scheme — `register_object_store` appears in exactly one file in this
    // workspace and it is the browser's — and the operator got
    // `Internal error: No suitable object store found for s3://…`, an error that
    // names a DataFusion API and nothing they wrote.
    //
    // This does not close the gap. It stops the gap being reported as an
    // internal error, and it says the part that is actually confusing: it
    // type-checked because a different engine read it.
    if let Some(uri) = fossil_df::program_sources(&db, file, &descriptor, connections)
        .iter()
        .map(|s| s.uri.clone())
        .find(|uri| uri.contains("://") && !uri.starts_with("file://"))
    {
        return Err(miette::miette!(
            "the DataFusion run path reads local files; source `{uri}` is not yet wired. \
             It type-checked because `check` introspects through DuckDB, which does read it"
        ));
    }
    // The host's byte seam for provider (RDF) sources. `provider_bindings` has
    // already put every URI through the anchor, so this call is idempotent on
    // what it is handed — it is here because a host that read a raw URI would
    // be the fourth resolution rule.
    let read_uri = |uri: &str| -> Result<String, String> {
        let locator = anchor.locator(uri);
        std::fs::read_to_string(&locator).map_err(|e| format!("read source `{locator}`: {e}"))
    };
    let graph = fossil_df::run_to_dir(
        &db,
        file,
        &descriptor,
        &dest_dir,
        connections,
        read_uri,
        memory_bytes,
    )
    .map_err(|e| miette::miette!("execute: {e}"))?;

    // W3.1b layout post-pass: replace the placeholder x/y/cluster_id with a real
    // WCC partition + deterministic placement, rewriting each vertex Parquet in
    // place (DuckDB — the one remaining native-runtime use on this path).
    //
    // **There is nothing to repoint any more, and that is the change.** A
    // `RunStatus` was built here BEFORE the pass and patched BY it, because it
    // named `vertex/<Type>.parquet` — the staged file whose deletion is the
    // pass's last act — and `fossil run --output-json` was handing keasy a path
    // to a file that had just stopped existing. The report is the manifest, and
    // the manifest has always declared the chunk prefix the pass writes into, so
    // the order of these two lines is no longer load-bearing.
    enrich_written_layout(&graph, &dest_dir, memory_bytes)?;

    Ok(RunReport::of(dest_url, &graph))
}

/// Run the W3 layout enrichment over the just-written `GraphAr` tree: for each
/// vertex type, point `DuckDB` at its `vertex/<Type>.parquet` plus the CSR Parquet
/// of any self-edge, and rewrite the placeholder `x/y/cluster_id` with a real
/// layout. Local-filesystem paths (the `run_to_dir` dest is a local dir).
///
/// `memory_bytes` is the run's budget again, applied to the second engine — the
/// pass is measured at 0.13 GiB net today, but it is the half of the write path
/// that reads back everything the first half wrote, and an unbounded `DuckDB`
/// targets ~80% of the machine.
fn enrich_written_layout(
    graph: &fossil_df::GraphArData,
    dest_dir: &Path,
    memory_bytes: Option<u64>,
) -> miette::Result<()> {
    // `memory_bytes` used to open a DuckDB connection here and cap it with
    // `apply_memory_budget` before handing it to the layout pass. The layout
    // reads and writes Parquet with `arrow-rs` now and holds no connection, so
    // there is nothing to cap — and nothing enforcing the budget either.
    //
    // **Measured, and it is 2.00 G through a 2 GiB cap**: one million nodes and
    // ten million edges, `--memory-gib 2`, the executor obeying at 2.21 G and
    // the layout then ending at 4.80 G. What costs it is the VERTEX read and
    // the gather, not the edge sort — the sort is +0.17 G of that, and this
    // comment used to say it was the whole of it. `design/one-engine.mdx`
    // carries the table.
    let _ = memory_bytes;

    let path_str = |rel: String| dest_dir.join(rel).to_string_lossy().into_owned();
    let adjacency = |e: &fossil_df::EdgeTable, file: &str| {
        path_str(format!(
            "edge/{}_{}_{}/{file}.parquet",
            e.src_type, e.label, e.dst_type
        ))
    };
    let targets: Vec<fossil_layout::layout::VertexLayoutTarget> = graph
        .schema
        .nodes
        .iter()
        .map(|node| {
            let self_edge_csr = graph
                .edges
                .iter()
                .filter(|e| e.src_type == node.label && e.dst_type == node.label)
                .map(|e| adjacency(e, "by_source"))
                .collect();
            fossil_layout::layout::VertexLayoutTarget {
                type_name: node.label.clone(),
                vertex_parquet: path_str(format!("vertex/{}.parquet", node.label)),
                // Trailing separator: the layout appends `chunk{k}.parquet`.
                chunk_prefix: path_str(format!("vertex/{}/", node.label)),
                // The same constant the manifest is written with, so the files
                // and the promise cannot drift apart.
                chunk_size: fossil_sinks::manifest::DEFAULT_CHUNK_SIZE,
                self_edge_csr,
            }
        })
        .collect();

    // The tile directories are not created here. `DuckDB`'s COPY writes a file
    // and not the directory above it, so somebody has to — and once the layout
    // emits edge tiles under a prefix it derives for itself, that somebody can
    // only be the layout. Two callers creating the same directory is one of them
    // being wrong about which directories exist.

    // Every adjacency file, both orientations, cross-type included — the layout
    // renumbers `dense_id`, and a file left out keeps ids that now belong to
    // somebody else. Enumerated here rather than derived there because this is
    // the side that has the schema.
    //
    // A missed file is a silent corruption, and what catches one is not this
    // comment: `tests/conformance.rs` assertion 4 reads every endpoint of every
    // adjacency file back with plain SQL and fails on a dense id no vertex
    // carries. A file this loop skipped keeps ids from before the renumbering
    // and dangles there.
    let adjacencies: Vec<fossil_layout::layout::AdjacencyTarget> = graph
        .edges
        .iter()
        .flat_map(|e| {
            use fossil_layout::layout::Endpoint;
            [
                (adjacency(e, "by_source"), Endpoint::Src),
                (adjacency(e, "by_target"), Endpoint::Dst),
            ]
            .map(
                |(parquet, ordered_by)| fossil_layout::layout::AdjacencyTarget {
                    parquet,
                    src_type: e.src_type.clone(),
                    dst_type: e.dst_type.clone(),
                    ordered_by,
                },
            )
        })
        .collect();

    fossil_layout::layout::enrich_layout(&targets, &adjacencies)
        .map_err(|e| miette::miette!("layout: {e}"))?;

    // The single-file vertex Parquet was this pass's input and nothing reads it
    // afterwards: the manifest points at the chunk prefix, and leaving it would
    // be a second copy of every vertex, stale the moment anything is re-run.
    //
    // A second loop stood here repointing a `RunStatus` at the prefix this pass
    // had just filled, because the status carried its own answer to «where are
    // this type's rows» and that answer was the deleted file. There is one
    // answer now — `VertexInfo::prefix`, written before the run started — so the
    // deletion is just a deletion.
    for target in &targets {
        std::fs::remove_file(&target.vertex_parquet)
            .map_err(|e| miette::miette!("remove staged {}: {e}", target.vertex_parquet))?;
    }
    Ok(())
}
