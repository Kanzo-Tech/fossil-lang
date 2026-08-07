//! `fossil-engine` — the native orchestration surface behind the `fossil`
//! binaries. It owns the compile→run pipeline (parse → `def_map` → typecheck →
//! `lower_to_mir` → `decompose_for_writer` → materialise `GraphAr`) plus `check` /
//! `refs` / `providers` / `catalog`, returning STRUCTURED data. The binary
//! (`fossil-cli`) is a thin shell: it parses args, reads files/stdin, calls these
//! functions, and renders the result (rustc-style miette for `check`, JSON/human
//! for the rest). Mirrors the rust-analyzer `ide`-façade / biome `service`
//! pattern — the orchestration is a named, testable crate, not a binary monolith.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-engine is native-only (depends on fossil-runtime which uses bundled DuckDB); \
     do not add it to the WASM CI gate"
);

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fossil_base::{Db, Diagnostic, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_graph_schema::Primitive;
use fossil_run_status::{ProviderInfo, RunStatus, SourceRefInfo};
use smol_str::SmolStr;

pub mod creds;
mod system;

pub use creds::{CatalogRequest, RunCreds};
use system::open_db;

// ===================================================================== providers

/// List the data-source providers fossil supports. Thin native wrapper over
/// [`fossil_lineage::providers`] (the shared, WASM-clean implementation — one
/// source of truth for both the CLI and the browser, ADR-0024).
#[must_use]
pub fn providers() -> Vec<ProviderInfo> {
    fossil_lineage::providers()
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
    // the browser runs over its in-memory db (ADR-0024).
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
}

/// Parse + type-check + lower `path`, draining the Salsa `Diagnostic`
/// accumulator across every mapping. Same pre-introspection as [`run`] so
/// `check` sees the same forward-propagated types the compiler will (no `@conn`
/// creds on `check`).
///
/// Drains from `lower_to_mir_pg` rather than `typecheck_mapping`: Salsa
/// accumulators are transitive, and lowering calls the typechecker, so this
/// yields the typecheck diagnostics PLUS the lowering ones without duplicating
/// either. Draining only the typechecker used to make `check` report `ok` for a
/// program `run` then refused — e.g. a mapping reading `from` a derived binding,
/// whose source cannot be resolved. `check` must not pass what `run` rejects.
///
/// # Errors
/// Returns a read error if `path` is unreadable.
pub fn check(path: &Path) -> miette::Result<CheckOutcome> {
    tracing::debug!(?path, "fossil check");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let (db, file) = open_db(text.clone(), path);
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    pre_introspect_and_register(db.system(), &text, source_dir, &HashMap::new());

    let def_map = fossil_hir::def_map::def_map(&db, file);
    let mappings = def_map.mappings(&db);

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for mapping in mappings {
        let _ = fossil_mir::lower_to_mir_pg(&db, *mapping);
        let diags = fossil_mir::lower_to_mir_pg::accumulated::<Diagnostic>(&db, *mapping);
        // Spans come out mapping-relative; the caller renders against the file.
        diagnostics.extend(fossil_hir::spans::rebase_to_file(
            &db,
            *mapping,
            diags.into_iter().cloned(),
        ));
    }
    Ok(CheckOutcome {
        source: text,
        diagnostics,
    })
}

// =============================================================== pre-introspection

/// Map a `DuckDB` column-type string onto the lattice — the native sibling of
/// `@fossil-lang/introspect`'s `duckdbTypeToFossilPrimitive`. A vocabulary the
/// engine reads and nobody else does, which is why it lives here and not on
/// [`Primitive`]; the xsd direction is the one the lattice owns.
fn duckdb_type_to_fossil_primitive(t: &str) -> Primitive {
    let upper = t.trim().to_ascii_uppercase();
    match upper.as_str() {
        "INTEGER" | "BIGINT" | "INT" | "SMALLINT" | "TINYINT" | "HUGEINT" => Primitive::Integer,
        "DOUBLE" | "FLOAT" | "REAL" => Primitive::Float,
        t if t.starts_with("DECIMAL") => Primitive::Float,
        "BOOLEAN" | "BOOL" => Primitive::Bool,
        "DATE" => Primitive::Date,
        "TIMESTAMP" | "DATETIME" => Primitive::DateTime,
        "TIME" => Primitive::Time,
        _ => Primitive::String,
    }
}

/// Scrape source-binding RHS source URLs from a `.fossil` file's text (regex,
/// v0.2 placeholder — Phase 14+ replaces with an AST walk). Mirrors the TS
/// `extractSourceRefs` so playground + CLI behave identically.
fn extract_source_refs(text: &str) -> Vec<(SmolStr, String)> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(\w[\w\d_]*)\s*:=\s*io\.(?:csv|json)\(\s*['"]([^'"]+)['"]"#)
            .expect("static regex")
    });
    re.captures_iter(text)
        .map(|c| {
            (
                SmolStr::from(c.get(1).unwrap().as_str()),
                c.get(2).unwrap().as_str().to_string(),
            )
        })
        .collect()
}

/// The token that decides whether a cached descriptor still describes its
/// source: the file's modification time in nanoseconds since the epoch, paired
/// with its byte length. Two `stat` fields, no read of the source itself — see
/// ADR-0050 for why the native host does not hash the bytes.
///
/// Returns `""` for anything this host cannot `stat` — an `http(s)://` or
/// `s3://` locator, or a path that does not exist. An empty token is never
/// fresh ([`fossil_descriptors_input::DescriptorCache::is_fresh`]), so those are
/// re-introspected on every compile. That is the honest answer for an object we
/// would have to make a network round trip to interrogate.
fn freshness_token(resolved: &str) -> String {
    let Ok(meta) = std::fs::metadata(resolved) else {
        return String::new();
    };
    let Ok(modified) = meta.modified() else {
        return String::new();
    };
    let Ok(since_epoch) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) else {
        return String::new();
    };
    format!(
        "mtime:{}.{:09}:size:{}",
        since_epoch.as_secs(),
        since_epoch.subsec_nanos(),
        meta.len()
    )
}

/// Turn a source URI as the program writes it into a locator `DuckDB` can read:
/// `@conn` aliases expand, a URL or absolute path passes through, and a
/// relative path anchors to the program's directory when that resolves to a
/// file that exists.
fn resolve_for_read(
    raw_uri: &str,
    source_dir: &Path,
    connections: &HashMap<String, creds::ConnectionCreds>,
) -> String {
    let url = resolve_source_uri(raw_uri, connections);
    let is_pass_through = url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("s3://")
        || Path::new(&url).is_absolute();
    if is_pass_through {
        return url;
    }
    let joined = source_dir.join(&url);
    if joined.exists() {
        joined.to_string_lossy().into_owned()
    } else {
        url
    }
}

/// Pre-introspect every source the program names and register an
/// [`InferredDescriptor`] on the host's descriptor cache BEFORE typecheck
/// (ADR-0037, keyed by URI since ADR-0050).
///
/// A source whose cached descriptor still carries the current
/// [`freshness_token`] is skipped — no `DESCRIBE`, no read. That is where the
/// cost is: programs are small and sources are not, so the introspection is
/// the expensive half of a compile and it is the half that rarely needs doing
/// twice.
///
/// Per-source failures are non-fatal — they log + skip; the compile may still
/// succeed with no forward propagation for that source.
fn pre_introspect_and_register(
    system: &dyn System,
    source_text: &str,
    source_dir: &Path,
    connections: &HashMap<String, creds::ConnectionCreds>,
) {
    let Some(cache) = system.descriptors() else {
        tracing::debug!("host keeps no descriptor cache; skipping pre-introspection");
        return;
    };

    // Opened on the first miss, not on entry. A compile whose sources are all
    // fresh must do no DuckDB work at all, and opening a connection is work.
    let mut conn: Option<duckdb::Connection> = None;

    for (source_name, raw_uri) in extract_source_refs(source_text) {
        let token = freshness_token(&resolve_for_read(&raw_uri, source_dir, connections));
        if cache.is_fresh(&raw_uri, &token) {
            tracing::debug!("`{raw_uri}` is unchanged since it was introspected; reusing");
            continue;
        }

        let conn = if let Some(c) = &conn {
            c
        } else {
            let opened = match duckdb::Connection::open_in_memory() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("DuckDB in-memory open failed; skipping pre-introspection: {e}");
                    return;
                }
            };
            if let Err(e) = apply_source_creds(&opened, connections) {
                tracing::warn!("applying source creds for pre-introspection failed: {e}");
            }
            conn.insert(opened)
        };

        // Resolved a second time deliberately: the token above is about the
        // bytes on disk, this is the string DuckDB reads, and conflating them
        // would make a `@conn` alias silently change meaning between the two.
        let resolved_path = resolve_for_read(&raw_uri, source_dir, connections);
        let escaped_path = resolved_path.replace('\'', "''");
        let sql = format!("DESCRIBE SELECT * FROM read_csv_auto('{escaped_path}')");
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "DESCRIBE prepare failed for source `{source_name}` (uri=`{raw_uri}`): {e}"
                );
                continue;
            }
        };
        let cols: Vec<InferredColumn> = match stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let typ: String = row.get(1)?;
            Ok(InferredColumn {
                name: SmolStr::from(name),
                primitive: duckdb_type_to_fossil_primitive(&typ),
            })
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!("DESCRIBE query_map failed for `{source_name}`: {e}");
                continue;
            }
        };
        cache.insert(InferredDescriptor {
            uri: SmolStr::from(raw_uri.as_str()),
            columns: cols,
            freshness_token: token,
        });
        tracing::debug!("introspected `{raw_uri}` for source `{source_name}`");
    }
}

// ================================================================== run pipeline

/// Resolve the program-resident OUTPUT descriptor: an `io.rdf(schema = …)` `ShEx`
/// IS the output graph's shape; no io.rdf schema ⇒ `AcceptAll`. The shape is
/// sourced from the PROGRAM, never a host flag (invariant #1). v1: one shape per
/// program (a second, different schema is rejected, not merged).
fn resolve_output_descriptor(
    db: &fossil_base::FossilDb,
    def_map: fossil_hir::def_map::DefMap<'_>,
    connections: &HashMap<String, creds::ConnectionCreds>,
    source_dir: &Path,
) -> miette::Result<OutputDescriptorKind> {
    let mut schema: Option<SmolStr> = None;
    for s in def_map.sources(db) {
        let is_provider = s
            .constructor
            .as_deref()
            .and_then(fossil_hir::stdlib::source_kind)
            .is_some_and(|k| k.lowering == fossil_hir::stdlib::SourceLowering::Provider);
        if !is_provider {
            continue;
        }
        let Some(arg) = s.schema_arg.as_ref() else {
            continue;
        };
        match &schema {
            Some(existing) if existing != arg => {
                return Err(miette::miette!(
                    "a program may declare only one io.rdf output shape (v1); found `{existing}` and `{arg}`"
                ));
            }
            _ => schema = Some(arg.clone()),
        }
    }

    let Some(schema) = schema else {
        return Ok(OutputDescriptorKind::ACCEPT_ALL_DEFAULT);
    };

    let locator = resolve_ref(schema.as_str(), connections, source_dir);
    let text = std::fs::read_to_string(&locator)
        .map_err(|e| miette::miette!("read io.rdf output shape `{locator}`: {e}"))?;
    let desc = fossil_descriptors_output::ShExDescriptor::from_reader(text.as_bytes())
        .map_err(|e| miette::miette!("parse io.rdf output shape `{locator}`: {e:?}"))?;
    Ok(OutputDescriptorKind::ShEx(desc))
}

/// Resolve a source-reference argument to a physical locator the cloud-capable
/// reader accepts — UNIFORMLY for every reference, so a `@conn` alias works in
/// ANY URI position. `@conn/path` → connection URL + path; a direct URL or
/// absolute path → itself; a relative path → anchored to the program's directory.
fn resolve_ref(
    raw: &str,
    connections: &HashMap<String, creds::ConnectionCreds>,
    source_dir: &Path,
) -> String {
    let resolved = resolve_source_uri(raw, connections);
    if resolved.contains("://") || Path::new(&resolved).is_absolute() {
        return resolved;
    }
    let joined = source_dir.join(&resolved);
    let anchored = if joined.exists() {
        joined
    } else {
        std::env::current_dir().unwrap_or_default().join(&resolved)
    };
    anchored.to_string_lossy().into_owned()
}

/// Resolve a `.fossil` source URI through the `--creds-stdin` connection map.
/// `@conn/path` → `<connection url>/path`; any other URI is returned verbatim.
/// Delegates to the shared rule [`fossil_df::resolve_source_uri`] (one
/// resolution authority across the native host + the browser executor),
/// projecting the creds map onto its name→base-URL view.
fn resolve_source_uri(raw: &str, connections: &HashMap<String, creds::ConnectionCreds>) -> String {
    let urls: HashMap<String, String> = connections
        .iter()
        .map(|(name, c)| (name.clone(), c.url.clone()))
        .collect();
    fossil_df::resolve_source_uri(raw, &urls)
}

/// Install each source connection's scoped read secret on `conn`, so a
/// `read_csv_auto` over a cloud `@conn` source authenticates. No-op for
/// connections without a secret (local / public-URL sources).
fn apply_source_creds(
    conn: &duckdb::Connection,
    connections: &HashMap<String, creds::ConnectionCreds>,
) -> miette::Result<()> {
    for (i, c) in connections.values().enumerate() {
        if let Some(spec) = &c.secret {
            let resolved =
                fossil_resolver::ResolvedPath::with_secret(&c.url, spec.to_cloud_secret());
            fossil_runtime::install_secret(conn, &resolved, &format!("__fossil_src_{i}"))
                .map_err(|e| miette::miette!("install source secret: {e}"))?;
        }
    }
    Ok(())
}

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
/// `dest_url`. Returns the [`RunStatus`] describing the output graph's structure.
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
pub fn run(
    path: &Path,
    dest_url: &str,
    creds: &RunCreds,
    memory_bytes: Option<u64>,
) -> miette::Result<RunStatus> {
    tracing::debug!(?path, dest_url, "fossil run");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let (db, file) = open_db(text.clone(), path);
    let source_dir = path.parent().unwrap_or_else(|| Path::new("."));
    // CSV type pre-introspection (DuckDB DESCRIBE) feeds the type-checker's
    // `source_row`, which the property-graph lowering reads for prop datatypes.
    pre_introspect_and_register(db.system(), &text, source_dir, &creds.connections);

    let def_map = fossil_hir::def_map::def_map(&db, file);
    if def_map.mappings(&db).is_empty() {
        return Err(miette::miette!("no mapping found in {}", path.display()));
    }

    // The program-resident output descriptor (ShEx) drives the PG edge/cardinality
    // classification inside `execute_graph` (via `apply_output_shape`).
    let descriptor = resolve_output_descriptor(&db, def_map, &creds.connections, source_dir)?;

    // The single execution path: lower to the property-graph MIR + execute on
    // DataFusion + write the GraphAr tree. The host's only job is the byte seam
    // for provider (RDF) sources — resolve the URI (`@conn` + relative) and read
    // it; object-store formats stream through the executor's filesystem store.
    let dest_dir = local_dest_dir(dest_url).ok_or_else(|| {
        miette::miette!("the DataFusion run path writes a local directory; cloud dest `{dest_url}` is not yet wired")
    })?;
    let read_uri = |uri: &str| -> Result<String, String> {
        let locator = resolve_ref(uri, &creds.connections, source_dir);
        std::fs::read_to_string(&locator).map_err(|e| format!("read source `{locator}`: {e}"))
    };
    // The name→base-URL ref-map the executor resolves `@conn` source aliases
    // through (object-store + provider sources alike) — projected from the
    // `--creds-stdin` connections, the single resolution authority.
    let connections: std::collections::HashMap<String, String> = creds
        .connections
        .iter()
        .map(|(name, c)| (name.clone(), c.url.clone()))
        .collect();
    let graph = fossil_df::run_to_dir(
        &db,
        file,
        &descriptor,
        &dest_dir,
        &connections,
        read_uri,
        memory_bytes,
    )
    .map_err(|e| miette::miette!("execute: {e}"))?;

    // W3.1b layout post-pass: replace the placeholder x/y/cluster_id with a real
    // WCC partition + deterministic placement, rewriting each vertex Parquet in
    // place (DuckDB — the one remaining native-runtime use on this path).
    enrich_written_layout(&graph, &dest_dir, memory_bytes)?;

    Ok(graph.run_status(dest_url))
}

/// Run the W3 layout enrichment over the just-written GraphAr tree: for each
/// vertex type, point DuckDB at its `vertex/<Type>.parquet` plus the CSR Parquet
/// of any self-edge, and rewrite the placeholder x/y/cluster_id with a real
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
    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    if let Some(bytes) = memory_bytes {
        fossil_runtime::apply_memory_budget(&conn, bytes)
            .map_err(|e| miette::miette!("apply duckdb memory budget: {e}"))?;
    }

    let path_str = |rel: String| dest_dir.join(rel).to_string_lossy().into_owned();
    let adjacency = |e: &fossil_df::EdgeTable, file: &str| {
        path_str(format!(
            "edge/{}_{}_{}/{file}.parquet",
            e.src_type, e.label, e.dst_type
        ))
    };
    let targets: Vec<fossil_runtime::layout::VertexLayoutTarget> = graph
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
            fossil_runtime::layout::VertexLayoutTarget {
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
    // the side that has the schema: a missed file is a silent corruption, so
    // naming the set is the caller's job and not a guess.
    let adjacencies: Vec<fossil_runtime::layout::AdjacencyTarget> = graph
        .edges
        .iter()
        .flat_map(|e| {
            use fossil_runtime::layout::Endpoint;
            [
                (adjacency(e, "by_source"), Endpoint::Src),
                (adjacency(e, "by_target"), Endpoint::Dst),
            ]
            .map(
                |(parquet, ordered_by)| fossil_runtime::layout::AdjacencyTarget {
                    parquet,
                    src_type: e.src_type.clone(),
                    dst_type: e.dst_type.clone(),
                    ordered_by,
                },
            )
        })
        .collect();

    fossil_runtime::layout::enrich_layout(&conn, &targets, &adjacencies)
        .map_err(|e| miette::miette!("layout: {e}"))?;

    // The single-file vertex Parquet was this pass's input and nothing reads it
    // afterwards: the manifest points at the chunk prefix, and leaving it would
    // be a second copy of every vertex, stale the moment anything is re-run.
    for target in &targets {
        std::fs::remove_file(&target.vertex_parquet)
            .map_err(|e| miette::miette!("remove staged {}: {e}", target.vertex_parquet))?;
    }
    Ok(())
}

/// Materialise a DCAT-AP catalog graph from a [`CatalogRequest`]. The catalog's
/// shape lives in fossil; the host supplies governance values + dataset
/// structure. The catalog is "just another graph": built from literal rows and
/// written by the SAME GraphAr path as [`run`] — `fossil_df` (Arrow), no DuckDB
/// executor, no second materialiser.
///
/// # Errors
/// Returns a write error.
pub fn catalog(dest_url: &str, req: &CatalogRequest) -> miette::Result<RunStatus> {
    let dest_dir = local_dest_dir(dest_url).ok_or_else(|| {
        miette::miette!(
            "the catalog writes a local directory; cloud dest `{dest_url}` is not yet wired"
        )
    })?;
    let graph = fossil_df::catalog::build_catalog_graph(&req.catalog);
    graph
        .write_to_dir(&dest_dir)
        .map_err(|e| miette::miette!("write catalog: {e}"))?;
    // No budget: a catalog is the governance rows the host just handed over on
    // stdin, so its size is the payload's and declaring a limit for it would be
    // a number with nothing to bound.
    enrich_written_layout(&graph, &dest_dir, None)?;
    Ok(graph.run_status(dest_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conns(pairs: &[(&str, &str)]) -> HashMap<String, creds::ConnectionCreds> {
        pairs
            .iter()
            .map(|(name, url)| {
                (
                    (*name).to_string(),
                    creds::ConnectionCreds {
                        url: (*url).to_string(),
                        secret: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn resolves_at_conn_to_connection_url() {
        let c = conns(&[("sales", "s3://bucket/prefix")]);
        assert_eq!(
            resolve_source_uri("@sales/2024/orders.csv", &c),
            "s3://bucket/prefix/2024/orders.csv"
        );
    }

    #[test]
    fn collapses_slashes_at_the_join() {
        let c = conns(&[("sales", "s3://bucket/prefix/")]);
        assert_eq!(
            resolve_source_uri("@sales/x.csv", &c),
            "s3://bucket/prefix/x.csv"
        );
    }

    #[test]
    fn passes_through_direct_urls_and_paths() {
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(
            resolve_source_uri("s3://other/x.csv", &c),
            "s3://other/x.csv"
        );
        assert_eq!(
            resolve_source_uri("examples/users.csv", &c),
            "examples/users.csv"
        );
    }

    #[test]
    fn unknown_connection_passes_through_verbatim() {
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(resolve_source_uri("@missing/x.csv", &c), "@missing/x.csv");
    }

    /// ADR-0050's done-when, counted rather than timed: changing the CSV and
    /// re-running re-introspects; not changing it does not.
    ///
    /// `registrations()` moves only when a `DESCRIBE` actually ran, so the
    /// assertion is on the number of reads of the source and not on how long
    /// the second call took. The third write adds a column, which moves the
    /// size as well as the mtime — the token is both, so the test does not
    /// depend on the filesystem's clock resolution.
    #[test]
    fn a_source_is_re_introspected_when_it_changes_and_not_when_it_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("users.csv");
        std::fs::write(&csv, "id,name\n1,ada\n").expect("write csv");
        let program = "users := io.csv(\"users.csv\")\n";
        let no_creds = HashMap::new();

        let system = system::EngineSystem::for_program_dir(dir.path());
        let cache = system.descriptors().expect("the engine keeps a table");

        pre_introspect_and_register(&system, program, dir.path(), &no_creds);
        assert_eq!(
            cache.registrations(),
            1,
            "the first compile reads the source"
        );
        assert_eq!(cache.get("users.csv").expect("registered").columns.len(), 2);

        pre_introspect_and_register(&system, program, dir.path(), &no_creds);
        assert_eq!(
            cache.registrations(),
            1,
            "an untouched source must not be read a second time"
        );

        std::fs::write(&csv, "id,name,email\n1,ada,ada@example.org\n").expect("rewrite csv");
        pre_introspect_and_register(&system, program, dir.path(), &no_creds);
        assert_eq!(
            cache.registrations(),
            2,
            "a changed source must be read again"
        );
        assert_eq!(
            cache.get("users.csv").expect("registered").columns.len(),
            3,
            "and the new column is visible to the checker"
        );
    }

    /// The key is the URI, so two bindings over one file cost one read — the
    /// case a binding-name key charged twice for.
    #[test]
    fn two_bindings_over_one_file_introspect_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("u.csv"), "id\n1\n").expect("write csv");
        let program = "a := io.csv(\"u.csv\")\nb := io.csv(\"u.csv\")\n";

        let system = system::EngineSystem::for_program_dir(dir.path());
        let cache = system.descriptors().expect("the engine keeps a table");
        pre_introspect_and_register(&system, program, dir.path(), &HashMap::new());

        assert_eq!(cache.registrations(), 1);
        assert_eq!(cache.len(), 1);
    }

    /// A source this host cannot `stat` gets an empty token, and an empty token
    /// is never fresh — so a remote object is re-introspected rather than
    /// trusted. The assertion is on the token, since the DESCRIBE of an
    /// unreachable URL fails and registers nothing.
    #[test]
    fn a_locator_that_cannot_be_stat_ed_yields_no_token() {
        assert_eq!(freshness_token("https://example.org/users.csv"), "");
        assert_eq!(freshness_token("/nonexistent/users.csv"), "");
    }

    #[test]
    fn the_token_moves_when_the_file_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("u.csv");
        std::fs::write(&csv, "id\n1\n").expect("write");
        let path = csv.to_string_lossy().into_owned();
        let first = freshness_token(&path);
        assert!(!first.is_empty(), "a local file has a token");
        assert_eq!(first, freshness_token(&path), "and it is stable");

        std::fs::write(&csv, "id,name\n1,ada\n").expect("rewrite");
        assert_ne!(first, freshness_token(&path));
    }
}
