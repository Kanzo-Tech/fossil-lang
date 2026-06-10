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
use fossil_run_status::{ProviderInfo, RefRole, RunStatus, SourceRefInfo};
use smol_str::SmolStr;

pub mod creds;
mod system;

pub use creds::{CatalogRequest, RunCreds};
use system::open_db;

// ===================================================================== providers

/// List the data-source providers fossil supports, derived from
/// [`fossil_registry::SOURCE_KINDS`] (the W1 single source of truth — native
/// readers AND external providers like `rdf`). Sorted for a deterministic order.
#[must_use]
pub fn providers() -> Vec<ProviderInfo> {
    use fossil_registry::SOURCE_KINDS;
    use fossil_run_status::ProviderKind;

    let mut providers: Vec<ProviderInfo> = SOURCE_KINDS
        .iter()
        .map(|k| ProviderInfo {
            name: k.short_name.to_string(),
            extensions: k.extensions.iter().map(|e| (*e).to_string()).collect(),
            kind: ProviderKind::Data,
        })
        .collect();
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    providers
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
    /// Split a raw reference into its `@conn` alias + path, or `None` + the whole
    /// locator (reports the ALIAS rather than the resolved URL).
    fn parse_ref(raw: &str, role: RefRole) -> SourceRefInfo {
        match raw.strip_prefix('@').and_then(|r| r.split_once('/')) {
            Some((conn, path)) => SourceRefInfo {
                connection: Some(conn.to_string()),
                path: path.to_string(),
                role,
            },
            None => SourceRefInfo {
                connection: None,
                path: raw.to_string(),
                role,
            },
        }
    }

    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let (db, file) = open_db(text, path);
    let def_map = fossil_hir::def_map::def_map(&db, file);

    // A destructuring `{ A, B } := io.rdf(uri, schema = "x")` expands to one
    // SourceEntry per member sharing the same uri + schema, so dedup identical
    // refs — a job's lineage is the DISTINCT (data, schema) it reads.
    let mut refs: Vec<SourceRefInfo> = Vec::new();
    for s in def_map.sources(&db) {
        if let Some(uri) = s.uri.as_deref() {
            let r = parse_ref(uri, RefRole::Data);
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
        if let Some(schema) = s.schema_arg.as_deref() {
            let r = parse_ref(schema, RefRole::Schema);
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }
    Ok(refs)
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

/// Parse + type-check `path`, draining the Salsa `Diagnostic` accumulator across
/// every mapping. Same pre-introspection as [`run`] so `check` sees the same
/// forward-propagated types the compiler will (no `@conn` creds on `check`).
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
        let _ = fossil_hir::check::typecheck_mapping(&db, *mapping);
        let diags = fossil_hir::check::typecheck_mapping::accumulated::<Diagnostic>(&db, *mapping);
        diagnostics.extend(diags.into_iter().cloned());
    }
    Ok(CheckOutcome {
        source: text,
        diagnostics,
    })
}

// =============================================================== pre-introspection

/// Map a `DuckDB` column-type string to the canonical Fossil Primitive name
/// (matches `fossil-hir::infer::primitive_from_name` exactly).
fn duckdb_type_to_fossil_primitive(t: &str) -> &'static str {
    let upper = t.trim().to_ascii_uppercase();
    match upper.as_str() {
        "INTEGER" | "BIGINT" | "INT" | "SMALLINT" | "TINYINT" | "HUGEINT" => "Integer",
        "DOUBLE" | "FLOAT" | "REAL" => "Float",
        t if t.starts_with("DECIMAL") => "Float",
        "BOOLEAN" | "BOOL" => "Bool",
        "DATE" => "Date",
        "TIMESTAMP" | "DATETIME" => "DateTime",
        "TIME" => "Time",
        _ => "String",
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

/// Pre-introspect every source binding and register an [`InferredDescriptor`] on
/// the [`System`] BEFORE typecheck (ADR-0037). Per-source failures are non-fatal
/// — they log + skip; the compile may still succeed with no forward propagation
/// for that source.
fn pre_introspect_and_register(
    system: &dyn System,
    source_text: &str,
    source_dir: &Path,
    connections: &HashMap<String, creds::ConnectionCreds>,
) {
    let conn = match duckdb::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("DuckDB in-memory open failed; skipping pre-introspection: {e}");
            return;
        }
    };
    let _ = fossil_runtime::apply_resource_limits(&conn);
    if let Err(e) = apply_source_creds(&conn, connections) {
        tracing::warn!("applying source creds for pre-introspection failed: {e}");
    }
    for (source_name, url) in extract_source_refs(source_text) {
        let url = resolve_source_uri(url.as_str(), connections);
        let url_str = url.as_str();
        let is_pass_through = url_str.starts_with("http://")
            || url_str.starts_with("https://")
            || url_str.starts_with("s3://")
            || Path::new(url_str).is_absolute();
        let resolved_path = if is_pass_through {
            url_str.to_string()
        } else {
            let joined = source_dir.join(url_str);
            if joined.exists() {
                joined.to_string_lossy().into_owned()
            } else {
                url_str.to_string()
            }
        };

        let escaped_path = resolved_path.replace('\'', "''");
        let sql = format!("DESCRIBE SELECT * FROM read_csv_auto('{escaped_path}')");
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "DESCRIBE prepare failed for source `{source_name}` (url=`{url}`): {e}"
                );
                continue;
            }
        };
        let cols: Vec<InferredColumn> = match stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let typ: String = row.get(1)?;
            Ok(InferredColumn {
                name: SmolStr::from(name),
                primitive: SmolStr::from(duckdb_type_to_fossil_primitive(&typ)),
            })
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!("DESCRIBE query_map failed for `{source_name}`: {e}");
                continue;
            }
        };
        let descriptor = InferredDescriptor {
            source_name: source_name.clone(),
            columns: cols,
            content_hash: String::new(),
        };
        system.register_inferred_descriptor(descriptor);
        tracing::debug!("pre-registered InferredDescriptor for `{source_name}`");
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
            .and_then(fossil_registry::source_kind)
            .is_some_and(|k| k.lowering == fossil_registry::SourceLowering::Provider);
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

/// Open the in-memory `DuckDB` connection for a run with the source connections'
/// read credentials installed (scoped `CREATE SECRET` per `@conn`). The single
/// authenticated connection that READS every reference and WRITES the output.
fn open_run_conn(
    source_creds: &HashMap<String, creds::ConnectionCreds>,
) -> miette::Result<duckdb::Connection> {
    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    fossil_runtime::apply_resource_limits(&conn)
        .map_err(|e| miette::miette!("apply duckdb resource limits: {e}"))?;
    apply_source_creds(&conn, source_creds)?;
    Ok(conn)
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
fn resolve_source_uri(raw: &str, connections: &HashMap<String, creds::ConnectionCreds>) -> String {
    let Some((conn_name, path)) = raw.strip_prefix('@').and_then(|r| r.split_once('/')) else {
        return raw.to_string(); // not an @conn reference — pass through
    };
    connections.get(conn_name).map_or_else(
        || raw.to_string(),
        |c| {
            format!(
                "{}/{}",
                c.url.trim_end_matches('/'),
                path.trim_start_matches('/')
            )
        },
    )
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
/// # Errors
/// Returns a compile, read, or materialisation error.
pub fn run(path: &Path, dest_url: &str, creds: &RunCreds) -> miette::Result<RunStatus> {
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
    let graph = fossil_df::run_to_dir(&db, file, &descriptor, &dest_dir, read_uri)
        .map_err(|e| miette::miette!("execute: {e}"))?;

    // W3.1b layout post-pass: replace the placeholder x/y/cluster_id with a real
    // WCC partition + deterministic placement, rewriting each vertex Parquet in
    // place (DuckDB — the one remaining native-runtime use on this path).
    enrich_written_layout(&graph, &dest_dir)?;

    Ok(graph.run_status(dest_url))
}

/// Run the W3 layout enrichment over the just-written GraphAr tree: for each
/// vertex type, point DuckDB at its `vertex/<Type>.parquet` plus the CSR Parquet
/// of any self-edge, and rewrite the placeholder x/y/cluster_id with a real
/// layout. Local-filesystem paths (the `run_to_dir` dest is a local dir).
fn enrich_written_layout(graph: &fossil_df::GraphArData, dest_dir: &Path) -> miette::Result<()> {
    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    fossil_runtime::apply_resource_limits(&conn)
        .map_err(|e| miette::miette!("apply duckdb resource limits: {e}"))?;

    let path_str = |rel: String| dest_dir.join(rel).to_string_lossy().into_owned();
    let targets: Vec<fossil_runtime::layout::VertexLayoutTarget> = graph
        .schema
        .nodes
        .iter()
        .map(|node| {
            let self_edge_csr = graph
                .edges
                .iter()
                .filter(|e| e.src_type == node.label && e.dst_type == node.label)
                .map(|e| {
                    path_str(format!(
                        "edge/{}_{}_{}/by_source.parquet",
                        e.src_type, e.label, e.dst_type
                    ))
                })
                .collect();
            fossil_runtime::layout::VertexLayoutTarget {
                vertex_parquet: path_str(format!("vertex/{}.parquet", node.label)),
                self_edge_csr,
            }
        })
        .collect();
    fossil_runtime::layout::enrich_layout(&conn, &targets)
        .map_err(|e| miette::miette!("layout: {e}"))
}

/// Materialise a DCAT-AP catalog graph from a [`CatalogRequest`]. The catalog's
/// shape lives in fossil; the host supplies governance values + dataset
/// structure. Written by the same path as [`run`] (sources are inline `VALUES`).
///
/// # Errors
/// Returns a materialisation error.
pub fn catalog(dest_url: &str, req: &CatalogRequest) -> miette::Result<RunStatus> {
    let (prelude_sql, sink_plan) = fossil_sinks::catalog::build_catalog_sink_plan(&req.catalog);
    let conn = open_run_conn(&HashMap::new())?;
    materialize(
        &conn,
        &prelude_sql,
        &sink_plan,
        dest_url,
        req.dest.secret.as_ref().map(creds::SecretSpec::to_cloud_secret),
        |_conn| Ok(()),
    )
}

/// Materialise a [`SinkPlan`] to `dest_url` via the W0b writer + layout pass, and
/// return the [`RunStatus`] describing the result. The single `GraphAr` write path
/// shared by [`run`] (data graph) and [`catalog`] (DCAT-AP graph). `before_prelude`
/// is the seam where external source providers materialise their relations.
#[allow(clippy::too_many_lines)] // one linear write→layout→status path; clearer whole
fn materialize(
    conn: &duckdb::Connection,
    prelude_sql: &str,
    sink_plan: &fossil_sinks::decomp::SinkPlan,
    dest_url: &str,
    dest_secret: Option<fossil_resolver::CloudSecret>,
    before_prelude: impl FnOnce(&duckdb::Connection) -> miette::Result<()>,
) -> miette::Result<RunStatus> {
    let write_options = fossil_sinks::writer::WriteOptions::default();
    let write_plan =
        fossil_sinks::writer::plan_writes_from_sink_plan(sink_plan, dest_url, &write_options)
            .map_err(|e| miette::miette!("plan_writes: {e}"))?;
    let manifests = fossil_sinks::writer::plan_manifests_from_sink_plan(sink_plan, &write_options)
        .map_err(|e| miette::miette!("plan_manifests: {e}"))?;

    before_prelude(conn)?;
    conn.execute_batch(prelude_sql)
        .map_err(|e| miette::miette!("create source views: {e}"))?;

    let resolved = dest_secret.map_or_else(
        || fossil_resolver::ResolvedPath::new(dest_url),
        |secret| fossil_resolver::ResolvedPath::with_secret(dest_url, secret),
    );

    // Local dests need their directory tree pre-created (DuckDB COPY won't mkdir).
    if let Some(dest_dir) = local_dest_dir(dest_url) {
        std::fs::create_dir_all(&dest_dir).map_err(|e| miette::miette!("create dest dir: {e}"))?;
        for s in &write_plan.vertex_statements {
            if let Some(parent) = dest_dir.join(&s.rel_path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| miette::miette!("create vertex dir: {e}"))?;
            }
        }
        for s in &write_plan.edge_statements {
            if let Some(parent) = dest_dir.join(&s.csr_rel_path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| miette::miette!("create edge dir: {e}"))?;
            }
        }
    }

    // YAML manifests go through DuckDB too (the SINGLE byte-writer for Parquet +
    // YAML). Strip the trailing newline for a byte-exact file.
    let write_yaml = |rel_path: &str, content: &str| -> Result<(), String> {
        let url = resolved.join(rel_path);
        let body = content.strip_suffix('\n').unwrap_or(content);
        let sql = format!(
            "COPY (SELECT '{}' AS x) TO '{}' (FORMAT csv, HEADER false, QUOTE '', DELIMITER ',')",
            body.replace('\'', "''"),
            url.url().replace('\'', "''"),
        );
        conn.execute_batch(&sql).map_err(|e| e.to_string())
    };

    fossil_runtime::materialize_graph_ar(conn, &write_plan, &manifests, &resolved, write_yaml)
        .map_err(|e| miette::miette!("materialize: {e}"))?;

    // W3.1b — real WCC partition + deterministic layout per vertex type.
    let layout_targets: Vec<fossil_runtime::layout::VertexLayoutTarget> = write_plan
        .vertex_statements
        .iter()
        .map(|vstmt| {
            let vtype = vstmt.type_name.as_str();
            let self_edge_csr = write_plan
                .edge_statements
                .iter()
                .zip(&manifests.edges)
                .filter(|(_, em)| em.edge_info.src_type == vtype && em.edge_info.dst_type == vtype)
                .map(|(es, _)| resolved.join(&es.csr_rel_path).url().to_string())
                .collect();
            fossil_runtime::layout::VertexLayoutTarget {
                vertex_parquet: resolved.join(&vstmt.rel_path).url().to_string(),
                self_edge_csr,
            }
        })
        .collect();
    fossil_runtime::layout::enrich_layout(conn, &layout_targets)
        .map_err(|e| miette::miette!("layout: {e}"))?;

    // Build the RunStatus describing the output graph's STRUCTURE (the caller
    // serialises it for the host or summarises it for a human). `count(*)` reads
    // the Parquet footer metadata — fast + cloud-safe.
    let count_rows = |rel_path: &str| -> Option<i64> {
        let url = resolved.join(rel_path);
        conn.query_row(
            &format!(
                "SELECT count(*) FROM read_parquet('{}')",
                url.url().replace('\'', "''")
            ),
            [],
            |r| r.get::<_, i64>(0),
        )
        .ok()
    };

    let vertices: Vec<fossil_run_status::VertexStatus> = write_plan
        .vertex_statements
        .iter()
        .zip(&sink_plan.vertices)
        .map(|(vstmt, vtable)| fossil_run_status::VertexStatus {
            vertex_type: vstmt.type_name.clone(),
            rdf_type: vtable.rdf_type.clone(),
            file: vstmt.rel_path.clone(),
            count: count_rows(&vstmt.rel_path),
            columns: vtable
                .properties
                .iter()
                .map(|p| fossil_run_status::ColumnStatus {
                    name: p.name.clone(),
                    data_type: p.data_type.clone(),
                    rdf_uri: p.rdf_uri.clone(),
                    xsd_datatype: p.xsd_datatype.clone(),
                })
                .collect(),
        })
        .collect();

    let edges: Vec<fossil_run_status::EdgeStatus> = write_plan
        .edge_statements
        .iter()
        .zip(&manifests.edges)
        .map(|(estmt, em)| fossil_run_status::EdgeStatus {
            edge_type: em.edge_info.edge_type.clone(),
            src_type: em.edge_info.src_type.clone(),
            dst_type: em.edge_info.dst_type.clone(),
            by_source: estmt.csr_rel_path.clone(),
            by_target: estmt.csc_rel_path.clone(),
            count: count_rows(&estmt.csr_rel_path),
        })
        .collect();

    Ok(RunStatus {
        version: fossil_run_status::WIRE_VERSION,
        dest: dest_url.to_string(),
        vertices,
        edges,
    })
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
        assert_eq!(resolve_source_uri("@sales/x.csv", &c), "s3://bucket/prefix/x.csv");
    }

    #[test]
    fn passes_through_direct_urls_and_paths() {
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(resolve_source_uri("s3://other/x.csv", &c), "s3://other/x.csv");
        assert_eq!(resolve_source_uri("examples/users.csv", &c), "examples/users.csv");
    }

    #[test]
    fn unknown_connection_passes_through_verbatim() {
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(resolve_source_uri("@missing/x.csv", &c), "@missing/x.csv");
    }
}
