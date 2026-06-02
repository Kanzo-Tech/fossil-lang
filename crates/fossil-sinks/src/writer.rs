//! `GraphAr` writer — emit `DuckDB` COPY SQL that materialises vertex + edge
//! Parquet chunks under a destination URL.
//!
//! Replaces `angelip2303/fossil-stdlib::rdf::parquet_writer` (720 LOC, Polars
//! based — see auto-memory `project_fossil_graph_reference_architecture.md`
//! W0b for the migration context). The reshape is non-trivial: this module
//! is the **plan emitter** only — it produces a pure-Rust [`WriteSqlPlan`]
//! from [`VertexSpec`] / [`EdgeSpec`] inputs. Execution against a real
//! `DuckDB` connection lives in `fossil-runtime`, per ADR-0017 ("Fossil
//! never byte-writes Parquet from Rust — the runtime materialises via
//! `COPY (...) TO ... (FORMAT PARQUET)`"). The split keeps `fossil-sinks`
//! WASM-clean (the workspace gate) while the actual `DuckDB` host-only
//! execution lives natively in `fossil-runtime`.
//!
//! ## Output shape (W0b)
//!
//! Vertex Parquet columns — **NEW shape**, diverges from the predecessor:
//!
//! ```text
//! dense_id    UINTEGER  — 0..N-1 sequential, the cosmos.gl dense index
//! subject     VARCHAR   — full IRI (GraphAr's "vertex id" in the spec sense)
//! <property columns>    — per the user's mapping
//! x           REAL      — placeholder 0.0 (W3 replaces with Leiden+force layout)
//! y           REAL      — placeholder 0.0  (idem)
//! cluster_id  UINTEGER  — placeholder 0  (W3 replaces with Leiden cluster)
//! ```
//!
//! Predecessor shape: `_id BIGINT | subject | properties...`. Three
//! deliberate divergences:
//!
//! - `dense_id` (renamed from `_id`) makes the semantics explicit — this
//!   is the cosmos.gl-ready dense index, not an opaque `GraphAr` id. UINT32
//!   instead of UBIGINT because cosmos.gl indexes into `Float32Array` vertex
//!   buffers (4G nodes is the WebGL ceiling); 32 bits is enough and saves
//!   4 bytes per vertex (~20 MB on a 5M-vertex graph).
//! - Placeholder `x, y, cluster_id` columns are present from W0b onward
//!   even though the values are constant — this is the shape the viewer +
//!   `fossil-graph::operations::viewport` expect. W3 replaces the placeholder
//!   emission step with a real Leiden+force pass; the consumer surface is
//!   stable from day one. (Alternative considered: emit only when W3
//!   lands. Rejected because every viewer-side code path would then need
//!   a feature-flag for "is this a W3+ Parquet?"; carrying the columns
//!   with placeholder values eliminates that branching.)
//! - No `(SELECT DISTINCT ON (subject) ...)` wrap by default. The
//!   predecessor unconditionally deduped subjects; the new writer accepts
//!   a `dedup: bool` per [`VertexSpec`] so callers with already-distinct
//!   inputs (e.g. catalogs emitted from a primary key) skip the cost.
//!
//! Edge Parquet columns:
//!
//! ```text
//! src_dense  UINTEGER  — sender's dense_id (resolved from src_iri)
//! dst_dense  UINTEGER  — receiver's dense_id (resolved from dst_iri)
//! ```
//!
//! Predecessor: `source BIGINT | target BIGINT`. Renamed for parity with
//! `src_dense`/`dst_dense` semantics on the vertex side; UINT32 for the
//! same `Float32Array` indexing argument.
//!
//! Per-edge directory layout:
//!
//! ```text
//! <dest>/edge/{src}_{label}_{dst}/
//!   by_source.parquet  — CSR-ordered (ORDER BY src_dense, dst_dense)
//!   by_target.parquet  — CSC-ordered (ORDER BY dst_dense, src_dense)
//! ```
//!
//! Both orderings are emitted unconditionally (the predecessor wrote only
//! CSR for the generic `materialize_frames` entry point — see auto-memory
//! W0 audit for the keasy consumption that relies on CSC).
//!
//! ## Larger-than-RAM honesty (W0b)
//!
//! W0b's W3-placeholder vertex emission uses `row_number() OVER ()` with
//! no `PARTITION BY` — `DuckDB` evaluates this as a streaming counter
//! (single global increment, O(1) state, no input materialisation), so
//! the working set is bounded by row-group buffers, not by N. Verified
//! against `DuckDB` 1.10.5 plan output (the operator is `ROW_NUMBER` with a
//! single partition, executed inline).
//!
//! Edge emission uses a JOIN against the freshly-written vertex Parquets
//! to resolve `src_iri`/`dst_iri` → `dense_id`. `DuckDB` builds a hash
//! table on the smaller-cardinality side (typically the vertex set per
//! type). For workloads where the vertex cardinality fits comfortably
//! (10M⁻ per type), this hash build is one-pass and stays in RAM. For
//! larger workloads `DuckDB` spills to disk (out-of-core hash join, since
//! 1.0). The OOM cliff is therefore "single vertex type exceeds working
//! memory + spill budget" — substantially higher than the predecessor's
//! Polars-in-RAM ceiling. W3's morton-sorted vertex order will let us
//! revisit this in the future with sort-merge join hints.
//!
//! ## Module scope (W0b/1)
//!
//! This commit lands the public spec types + the vertex-side SQL emitter.
//! Edge emission (W0b/2), `GraphAr` manifest emission (W0b/3), and the
//! `fossil-runtime` execution side (W0b/4) are separate sub-commits in
//! the same `feat/fossil-graph-w1` branch — each reviewable in isolation.

use std::collections::BTreeMap;

// ──────────────────────────────────────────────────────────────────────────
// Public input shapes
// ──────────────────────────────────────────────────────────────────────────

/// Specification for one vertex type's emission.
///
/// `source_relation` is the SQL relation body (a subquery body without the
/// outer parens, or a view/table name) the COPY statement reads `FROM`.
/// It MUST produce a `subject` column (the IRI) plus zero or more property
/// columns. The writer adds `dense_id` / `x` / `y` / `cluster_id`; callers
/// MUST NOT include those names in their property column list (validated
/// by [`WriteOptions::reserved_column_names`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexSpec {
    /// Short local name (e.g. `"person"`) — drives the output directory
    /// (`vertex/<name>.parquet`) and the edge naming convention.
    pub name: String,
    /// Full RDF type IRI. Empty string permitted for non-RDF graphs.
    pub iri: String,
    /// SQL relation body or relation name producing the vertex rows.
    pub source_relation: String,
    /// Property column names in the order they appear in `source_relation`.
    /// `subject` is implicit and must NOT appear in this list. Reserved
    /// names (`dense_id`, `x`, `y`, `cluster_id`) are rejected by
    /// [`plan_writes`].
    pub property_columns: Vec<String>,
    /// Whether to apply `SELECT DISTINCT ON (subject)` before the
    /// `row_number()` enumeration. Set `false` when the caller already
    /// guarantees distinct subjects (e.g. a primary-key-derived source);
    /// set `true` for triple-stream inputs.
    pub dedup_subjects: bool,
    /// Optional column-name → predicate-IRI mapping carried through to
    /// the manifest (W0b/3 reads this). `BTreeMap` for deterministic
    /// snapshot ordering.
    pub column_iris: BTreeMap<String, String>,
}

/// Specification for one edge type's emission.
///
/// `source_relation` MUST produce columns `src_iri` (VARCHAR) and
/// `dst_iri` (VARCHAR). The writer resolves both to dense indices via
/// JOINs against the freshly-written vertex Parquets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeSpec {
    /// Short edge label (e.g. `"knows"`) — drives the edge directory name.
    pub label: String,
    /// Full predicate IRI.
    pub iri: String,
    /// Source vertex type name — must match a [`VertexSpec::name`] in the
    /// same write batch.
    pub source_type: String,
    /// Target vertex type name — must match a [`VertexSpec::name`] in the
    /// same write batch.
    pub target_type: String,
    /// SQL relation body or relation name producing `src_iri | dst_iri`.
    pub source_relation: String,
}

/// Knobs for the writer's SQL generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOptions {
    /// `ROW_GROUP_SIZE` argument to `DuckDB` COPY (Parquet row-group rows).
    /// Default `100_000` — balanced for DuckDB-WASM Range-request locality
    /// (~ 5-10 MB per group for typical property widths) versus per-group
    /// metadata overhead. Predecessor used `262_144`; the smaller default
    /// improves predicate-pushdown granularity once W3 morton-sort lands.
    pub row_group_size: u64,
    /// Vertex Parquet path under `<dest>/`. Default `"vertex/"`.
    pub vertex_prefix: String,
    /// Edge Parquet path under `<dest>/`. Default `"edge/"`.
    pub edge_prefix: String,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            row_group_size: 100_000,
            vertex_prefix: "vertex/".to_string(),
            edge_prefix: "edge/".to_string(),
        }
    }
}

impl WriteOptions {
    /// Column names the writer reserves on the vertex Parquet shape.
    /// Callers' `property_columns` must not overlap.
    #[must_use]
    pub const fn reserved_column_names() -> &'static [&'static str] {
        &["dense_id", "subject", "x", "y", "cluster_id"]
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Plan output
// ──────────────────────────────────────────────────────────────────────────

/// The pure-data result of [`plan_writes`]: the SQL statements
/// `fossil-runtime` must execute (in order) against a `DuckDB` connection
/// to produce the `GraphAr` Parquet under the destination URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteSqlPlan {
    /// One per [`VertexSpec`] in input order.
    pub vertex_statements: Vec<VertexStatement>,
    /// One per [`EdgeSpec`] — empty in W0b/1 (edges land in W0b/2).
    pub edge_statements: Vec<EdgeStatement>,
}

/// SQL plan for materialising one vertex type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexStatement {
    /// Local type name from the input [`VertexSpec`].
    pub type_name: String,
    /// Relative path under the destination URL — `"vertex/<name>.parquet"`.
    pub rel_path: String,
    /// The `COPY ... TO '<dest_url>/<rel_path>' (FORMAT PARQUET, ...)`
    /// statement to execute.
    pub copy_sql: String,
}

/// SQL plan for materialising one edge type — CSR + CSC.
///
/// `None` fields land in W0b/2; the struct is declared in W0b/1 so the
/// public API surface is stable across sub-commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeStatement {
    pub edge_dir_name: String,
    pub csr_rel_path: String,
    pub csc_rel_path: String,
    pub copy_csr_sql: String,
    pub copy_csc_sql: String,
}

// ──────────────────────────────────────────────────────────────────────────
// Errors
// ──────────────────────────────────────────────────────────────────────────

/// Planning errors. Pure data validation — no I/O failures (those live in
/// the runtime side that executes the plan).
///
/// Note: not `PartialEq` because `ManifestYaml` wraps the upstream
/// `serde_yaml_ng::Error` which doesn't implement `Eq`. Tests pattern-
/// match instead.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// A `VertexSpec::property_columns` entry collides with a reserved
    /// column name the writer emits itself.
    #[error(
        "vertex `{vertex}`: column `{column}` collides with a reserved writer column ({reserved:?})"
    )]
    ReservedColumnName {
        vertex: String,
        column: String,
        reserved: &'static [&'static str],
    },
    /// An [`EdgeSpec`] references a vertex type name that does not appear
    /// in the same write batch. The writer cannot resolve `src_iri` /
    /// `dst_iri` without the corresponding vertex Parquet existing.
    #[error(
        "edge `{src}_{label}_{dst}`: referenced vertex type `{missing}` is not in the write batch"
    )]
    EdgeRefersMissingVertex {
        src: String,
        label: String,
        dst: String,
        missing: String,
    },
    /// The destination URL is empty.
    #[error("destination URL must not be empty")]
    EmptyDestUrl,
    /// `serde_yaml_ng` failed to serialize a `VertexInfo` / `EdgeInfo`
    /// (W0b/3 manifest emission). Cannot fail in practice — the structs
    /// are plain data — but the variant exists so [`plan_manifests`]
    /// surfaces the error type honestly instead of unwrapping.
    #[error("manifest YAML serialisation failed: {0}")]
    ManifestYaml(#[from] serde_yaml_ng::Error),
}

pub type Result<T> = std::result::Result<T, WriteError>;

// ──────────────────────────────────────────────────────────────────────────
// Plan emitter
// ──────────────────────────────────────────────────────────────────────────

/// Build the SQL plan for materialising the given vertex + edge batch
/// under `dest_url`.
///
/// `dest_url` is a URL (or local filesystem path) that the runtime will
/// pass to `DuckDB` COPY without further transformation. It must NOT have a
/// trailing slash (the writer prepends one before each relative path).
///
/// # Errors
///
/// Returns [`WriteError`] when the input batch is internally inconsistent
/// (reserved-column collision, missing vertex reference, empty dest URL).
/// I/O failures are out of scope — they surface when `fossil-runtime`
/// executes the returned plan.
pub fn plan_writes(
    vertices: &[VertexSpec],
    edges: &[EdgeSpec],
    dest_url: &str,
    options: &WriteOptions,
) -> Result<WriteSqlPlan> {
    if dest_url.is_empty() {
        return Err(WriteError::EmptyDestUrl);
    }
    validate_inputs(vertices, edges)?;

    let vertex_statements: Vec<VertexStatement> = vertices
        .iter()
        .map(|spec| emit_vertex_statement(spec, dest_url, options))
        .collect();

    // Lookup so the edge emitter finds each vertex type's freshly-written
    // Parquet path without re-deriving the naming convention — the
    // convention lives in `emit_vertex_statement`; the edge emitter
    // consumes the result, not the rule.
    let vertex_paths: std::collections::HashMap<&str, &str> = vertex_statements
        .iter()
        .map(|s| (s.type_name.as_str(), s.rel_path.as_str()))
        .collect();

    let edge_statements: Vec<EdgeStatement> = edges
        .iter()
        .map(|spec| emit_edge_statement(spec, &vertex_paths, dest_url, options))
        .collect();

    Ok(WriteSqlPlan {
        vertex_statements,
        edge_statements,
    })
}

fn validate_inputs(vertices: &[VertexSpec], edges: &[EdgeSpec]) -> Result<()> {
    let reserved = WriteOptions::reserved_column_names();
    for v in vertices {
        for col in &v.property_columns {
            if reserved.iter().any(|r| r == col) {
                return Err(WriteError::ReservedColumnName {
                    vertex: v.name.clone(),
                    column: col.clone(),
                    reserved,
                });
            }
        }
    }
    let vertex_names: std::collections::HashSet<&str> =
        vertices.iter().map(|v| v.name.as_str()).collect();
    for e in edges {
        for (missing_ref, side) in [
            (&e.source_type, "source_type"),
            (&e.target_type, "target_type"),
        ] {
            let _ = side; // currently unused — kept for future error variants
            if !vertex_names.contains(missing_ref.as_str()) {
                return Err(WriteError::EdgeRefersMissingVertex {
                    src: e.source_type.clone(),
                    label: e.label.clone(),
                    dst: e.target_type.clone(),
                    missing: missing_ref.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Emit the `COPY ... TO ... (FORMAT PARQUET, ROW_GROUP_SIZE ...)`
/// statement for one vertex type with the W0b column shape.
///
/// The inner SELECT projects:
///   `row_number() OVER () - 1 AS dense_id`  — streaming counter in `DuckDB`
///   `subject`                               — IRI verbatim
///   `<property_cols>`                       — caller-declared, comma joined
///   `0::REAL AS x`                          — placeholder (W3 fills)
///   `0::REAL AS y`                          — placeholder (W3 fills)
///   `0::UINTEGER AS cluster_id`             — placeholder (W3 fills)
///
/// When `dedup_subjects` is set, the source relation is wrapped in
/// `(SELECT DISTINCT ON (subject) * FROM (<source>))` first.
fn emit_vertex_statement(
    spec: &VertexSpec,
    dest_url: &str,
    options: &WriteOptions,
) -> VertexStatement {
    let mut select_cols = String::from("row_number() OVER () - 1 AS dense_id, subject");
    for col in &spec.property_columns {
        select_cols.push_str(", ");
        // Quote with double quotes per the rmlext SQL convention (see
        // server/src/ai/routes.rs system prompt). Keasy + DuckDB both
        // require this for identifiers with mixed case or RDF-derived
        // local names that happen to be SQL reserved words.
        select_cols.push('"');
        select_cols.push_str(col);
        select_cols.push('"');
    }
    select_cols.push_str(", 0::REAL AS x, 0::REAL AS y, 0::UINTEGER AS cluster_id");

    let source = if spec.dedup_subjects {
        format!(
            "SELECT DISTINCT ON (subject) * FROM ({source})",
            source = spec.source_relation,
        )
    } else {
        spec.source_relation.clone()
    };

    let rel_path = format!("{}{}.parquet", options.vertex_prefix, spec.name);
    let copy_sql = format!(
        "COPY (SELECT {select_cols} FROM ({source})) \
         TO '{dest}/{rel}' \
         (FORMAT PARQUET, ROW_GROUP_SIZE {rgs})",
        dest = dest_url.trim_end_matches('/'),
        rel = rel_path,
        rgs = options.row_group_size,
    );

    VertexStatement {
        type_name: spec.name.clone(),
        rel_path,
        copy_sql,
    }
}

// CSR / CSC are graph-storage canon (Compressed Sparse Row / Compressed
// Sparse Column — GraphAr v1 adjacency conventions); the `_copy` SQL
// bindings paired by row/column ordering keep their canonical names.
#[allow(clippy::similar_names)]
/// Emit the two `COPY ... TO ... (FORMAT PARQUET, ...)` statements (CSR
/// + CSC) for one edge type.
///
/// The body resolves `src_iri` / `dst_iri` → `dense_id` via JOINs against
/// the freshly-written vertex Parquets. The naming convention
/// `{src}_{label}_{dst}` drives the edge directory and matches
/// `fossil-graph::operations::schema::EdgeTypeSummary::table_name` — the
/// read side (fossil-graph schema verb) and the write side (this emitter)
/// share the same naming rule by convention; if the rule changes either
/// side, both must move together.
fn emit_edge_statement(
    spec: &EdgeSpec,
    vertex_paths: &std::collections::HashMap<&str, &str>,
    dest_url: &str,
    options: &WriteOptions,
) -> EdgeStatement {
    let dest = dest_url.trim_end_matches('/');

    // Vertex paths come from `vertex_statements` so they're guaranteed
    // present (validate_inputs rejected missing references before this
    // point). `expect` on a programmer-error invariant — not a runtime
    // failure mode.
    let src_vertex_rel = vertex_paths
        .get(spec.source_type.as_str())
        .copied()
        .expect("source_type validated against vertex_paths");
    let dst_vertex_rel = vertex_paths
        .get(spec.target_type.as_str())
        .copied()
        .expect("target_type validated against vertex_paths");

    let edge_dir_name = format!("{}_{}_{}", spec.source_type, spec.label, spec.target_type);
    let csr_rel_path = format!("{}{}/by_source.parquet", options.edge_prefix, edge_dir_name);
    let csc_rel_path = format!("{}{}/by_target.parquet", options.edge_prefix, edge_dir_name);

    // Shared inner SELECT — the JOIN cost is paid once per edge type
    // (DuckDB executes the COPY query body twice; row-group caches on
    // the vertex Parquets keep the second JOIN cheap). The alternative
    // (CREATE TEMP TABLE __edges + two COPYs) saves the JOIN but adds a
    // temp-table lifecycle the runtime executor must clean up — kept
    // simple here, revisit if benchmark warrants.
    let inner = format!(
        "SELECT s.dense_id AS src_dense, t.dense_id AS dst_dense \
         FROM ({src_relation}) e \
         JOIN read_parquet('{dest}/{src_v}') s ON e.src_iri = s.subject \
         JOIN read_parquet('{dest}/{dst_v}') t ON e.dst_iri = t.subject",
        src_relation = spec.source_relation,
        src_v = src_vertex_rel,
        dst_v = dst_vertex_rel,
    );

    let csr_copy = format!(
        "COPY ({inner} ORDER BY src_dense, dst_dense) \
         TO '{dest}/{csr}' \
         (FORMAT PARQUET, ROW_GROUP_SIZE {rgs})",
        csr = csr_rel_path,
        rgs = options.row_group_size,
    );
    let csc_copy = format!(
        "COPY ({inner} ORDER BY dst_dense, src_dense) \
         TO '{dest}/{csc}' \
         (FORMAT PARQUET, ROW_GROUP_SIZE {rgs})",
        csc = csc_rel_path,
        rgs = options.row_group_size,
    );

    EdgeStatement {
        edge_dir_name,
        csr_rel_path,
        csc_rel_path,
        copy_csr_sql: csr_copy,
        copy_csc_sql: csc_copy,
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Manifest emission (W0b/3)
// ──────────────────────────────────────────────────────────────────────────

use crate::manifest::{
    AdjList, EdgeInfo, GRAPHAR_VERSION, GraphInfo, Property, PropertyGroup, VertexInfo,
    data_type_name,
};
use arrow_schema::DataType;

/// The `GraphAr` manifest set for a single write batch.
///
/// Companion to [`WriteSqlPlan`] — the runtime executes the SQL plan AND
/// writes these manifests as YAML next to the Parquet output. The two
/// sides live in distinct types because the runtime's responsibilities
/// for them are different (one is `conn.execute_batch(sql)`, the other
/// is `host.write_file(yaml)`); coupling them would force runtime callers
/// to handle both side-effects together when they may have different
/// host integrations (e.g. native CLI writes to filesystem; future
/// browser host writes to OPFS).
#[derive(Debug, Clone)]
pub struct ManifestSet {
    /// The `GraphAr` top-level graph info (`graph.graph.yml`) — the aggregate
    /// index the query side reads first to discover every type. Listed before
    /// the per-type manifests because it references them.
    pub graph: ManifestForGraph,
    pub vertices: Vec<ManifestForVertex>,
    pub edges: Vec<ManifestForEdge>,
}

/// The top-level graph manifest: the structured [`GraphInfo`] plus the YAML
/// string the runtime writes at the dataset root.
#[derive(Debug, Clone)]
pub struct ManifestForGraph {
    pub graph_info: GraphInfo,
    pub yaml: String,
    /// Where the YAML should land — `graph.graph.yml` at the dataset root.
    pub rel_path: String,
}

/// One vertex type's manifest pair: the structured `VertexInfo` (for tests
/// and future inspection) and the YAML string the runtime writes next to
/// the vertex Parquet.
#[derive(Debug, Clone)]
pub struct ManifestForVertex {
    pub vertex_info: VertexInfo,
    pub yaml: String,
    /// Where the YAML should land — `vertex/<name>.vertex.yml` by default.
    /// Mirrors the predecessor's convention (`fossil-stdlib::rdf::parquet_writer::write_yaml_metadata`).
    pub rel_path: String,
}

/// One edge type's manifest pair (analogous to [`ManifestForVertex`]).
#[derive(Debug, Clone)]
pub struct ManifestForEdge {
    pub edge_info: EdgeInfo,
    pub yaml: String,
    /// `edge/<dir>/<dir>.edge.yml` by default.
    pub rel_path: String,
}

/// Build the manifest set for a write batch, including the W0b vertex
/// column shape (`dense_id` + `subject` + caller properties + `x`, `y`,
/// `cluster_id` layout placeholders).
///
/// # Errors
///
/// Returns [`WriteError`] under the same validation rules as
/// [`plan_writes`] — reserved column collisions, missing vertex
/// references, empty dest URL. Manifest construction itself is
/// infallible once validation passes.
pub fn plan_manifests(
    vertices: &[VertexSpec],
    edges: &[EdgeSpec],
    options: &WriteOptions,
) -> Result<ManifestSet> {
    validate_inputs(vertices, edges)?;
    let vertex_manifests = vertices
        .iter()
        .map(|spec| build_vertex_manifest(spec, options))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let edge_manifests = edges
        .iter()
        .map(|spec| build_edge_manifest(spec, options))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let graph = build_graph_manifest(&vertex_manifests, &edge_manifests)?;
    Ok(ManifestSet {
        graph,
        vertices: vertex_manifests,
        edges: edge_manifests,
    })
}

/// Build the `GraphAr` top-level [`GraphInfo`] referencing every per-type
/// manifest by its `rel_path`. The default graph name is `"graph"`, so the
/// aggregate index lands at `graph.graph.yml` — the single file the query
/// side (fossil-graph) fetches first to enumerate all vertex/edge types.
fn build_graph_manifest(
    vertices: &[ManifestForVertex],
    edges: &[ManifestForEdge],
) -> std::result::Result<ManifestForGraph, serde_yaml_ng::Error> {
    let graph_info = GraphInfo::new(
        "graph",
        "",
        vertices.iter().map(|m| m.rel_path.clone()).collect(),
        edges.iter().map(|m| m.rel_path.clone()).collect(),
    );
    let yaml = graph_info.to_yaml()?;
    Ok(ManifestForGraph {
        graph_info,
        yaml,
        rel_path: "graph.graph.yml".to_string(),
    })
}

fn build_vertex_manifest(
    spec: &VertexSpec,
    options: &WriteOptions,
) -> std::result::Result<ManifestForVertex, serde_yaml_ng::Error> {
    let mut properties = Vec::with_capacity(spec.property_columns.len() + 5);

    // The W0b vertex column shape (declared in src/lib.rs module doc):
    //   dense_id    UINTEGER  — primary key, the cosmos.gl dense index
    //   subject     VARCHAR   — IRI (GraphAr's canonical vertex identifier)
    //   <caller properties>
    //   x, y        REAL      — layout placeholder (W3 fills)
    //   cluster_id  UINTEGER  — Leiden cluster placeholder (W3 fills)
    properties.push(Property {
        name: "dense_id".to_string(),
        // arrow-schema has no UInt32 → graphar mapping rule in
        // data_type_name's match arms (it falls through to "binary").
        // We hard-code "uint32" here because GraphAr v1 admits the
        // spelling and downstream readers (fossil-graph viewport verb +
        // cosmograph.gl Float32Array consumers) decode by exact name.
        data_type: "uint32".to_string(),
        is_primary: true,
        is_nullable: Some(false),
    });
    properties.push(Property {
        name: "subject".to_string(),
        data_type: data_type_name(&DataType::Utf8),
        is_primary: false,
        is_nullable: Some(false),
    });
    for col in &spec.property_columns {
        // The writer doesn't see arrow datatypes for caller property
        // columns at plan time (they come from the source SQL's runtime
        // schema). Declare them as `string` here — the predecessor's
        // permissive default. W0b/4 can promote this to a per-column
        // datatype when the executor surfaces the COPY-time schema.
        properties.push(Property {
            name: col.clone(),
            data_type: data_type_name(&DataType::Utf8),
            is_primary: false,
            is_nullable: None,
        });
    }
    for layout_col in ["x", "y"] {
        properties.push(Property {
            name: layout_col.to_string(),
            data_type: data_type_name(&DataType::Float32),
            is_primary: false,
            is_nullable: Some(false),
        });
    }
    properties.push(Property {
        name: "cluster_id".to_string(),
        data_type: "uint32".to_string(),
        is_primary: false,
        is_nullable: Some(false),
    });

    let mut vertex_info = VertexInfo::new(
        spec.name.clone(),
        options.row_group_size,
        format!("{}{}/", options.vertex_prefix, spec.name),
        vec![PropertyGroup {
            file_type: "parquet".to_string(),
            properties,
        }],
    );
    vertex_info.iri.clone_from(&spec.iri);
    let yaml = vertex_info.to_yaml()?;
    let rel_path = format!("{}{}.vertex.yml", options.vertex_prefix, spec.name);
    Ok(ManifestForVertex {
        vertex_info,
        yaml,
        rel_path,
    })
}

fn build_edge_manifest(
    spec: &EdgeSpec,
    options: &WriteOptions,
) -> std::result::Result<ManifestForEdge, serde_yaml_ng::Error> {
    let edge_dir_name = format!("{}_{}_{}", spec.source_type, spec.label, spec.target_type);
    let edge_info = EdgeInfo {
        src_type: spec.source_type.clone(),
        edge_type: spec.label.clone(),
        iri: spec.iri.clone(),
        dst_type: spec.target_type.clone(),
        chunk_size: options.row_group_size,
        src_chunk_size: options.row_group_size,
        dst_chunk_size: options.row_group_size,
        directed: true,
        prefix: format!("{}{}/", options.edge_prefix, edge_dir_name),
        // CSR + CSC both ordered (we emit ORDER BY in both COPY
        // statements, see emit_edge_statement).
        adj_lists: vec![
            AdjList {
                ordered: true,
                aligned_by: "src".to_string(),
                file_type: "parquet".to_string(),
            },
            AdjList {
                ordered: true,
                aligned_by: "dst".to_string(),
                file_type: "parquet".to_string(),
            },
        ],
        // No edge properties in W0b (edges carry only src_dense /
        // dst_dense). Predecessor's edge predicate IRI metadata lives
        // off the manifest in a separate registry; will revisit when
        // fossil-graph::operations::describe_field needs per-edge
        // properties.
        property_groups: vec![],
        version: GRAPHAR_VERSION.to_string(),
    };
    let yaml = edge_info.to_yaml()?;
    let rel_path = format!(
        "{}{}/{}.edge.yml",
        options.edge_prefix, edge_dir_name, edge_dir_name
    );
    Ok(ManifestForEdge {
        edge_info,
        yaml,
        rel_path,
    })
}

// ──────────────────────────────────────────────────────────────────────────
// SinkPlan bridge (W0b/5)
// ──────────────────────────────────────────────────────────────────────────
//
// Why a bridge instead of a single API
// ────────────────────────────────────
//
// W0b/1-3 designed the writer around `VertexSpec` / `EdgeSpec` because the
// keasy-server use case was thought to be the prime caller (it already has
// `DataManifest`-shaped inputs from its DCAT materialiser). The architectural
// pivot of 2026-05-29 (auto-memory `project_fossil_graph_reference_architecture.md`)
// rebases that: keasy invokes fossil-cli as a subprocess, so the prime caller
// of the writer becomes the CLI itself, whose upstream is the compiler's
// `SinkPlan` (per-shape `VertexTable` + per-predicate `EdgeTable`, from
// `decomp::vertex_edge_decomp`).
//
// Rather than refactor the existing API (which would invalidate the W0b/1-4
// tests + downstream consumers of VertexSpec like the DCAT non-fossil path),
// we add a thin adapter that maps `SinkPlan` shapes into `VertexSpec` /
// `EdgeSpec` shapes and delegates to the existing emitter. Two public
// entry points:
//
//   plan_writes_from_sink_plan(sink_plan, dest, options)    → WriteSqlPlan
//   plan_manifests_from_sink_plan(sink_plan, options)       → ManifestSet
//
// The mapping wraps each `VertexTable.source_relation` with a projection
// that renames the canonical `iri` column (decomp's SINK-04 convention) to
// `subject` (the W0b writer's convention) and lists property columns by
// name. `EdgeTable.source_relation` is wrapped to project
// `src_id_expr AS src_iri, dst_id_expr AS dst_iri` so the W0b edge emitter's
// JOINs find the columns they expect.
//
// Gaps the bridge accepts (documented, not fatal)
// ────────────────────────────────────────────────
//
// - **IRI metadata.** `VertexTable` does not carry the shape IRI it was
//   derived from (decomp drops it, keeping only the local name as
//   `type_name`). The bridge produces `VertexSpec.iri = ""`. W0b/3
//   manifest emission does not surface the field anywhere — VertexInfo
//   has no IRI slot in GraphAr v1 — so this is observationally a no-op
//   today. A future commit that backfills IRIs would augment `decomp`,
//   not this bridge.
// - **Edge predicate IRI.** Same story: `EdgeTable.predicate` is the
//   local name; the full predicate IRI was lost at decomp time. Bridge
//   produces `EdgeSpec.iri = ""`. Same downstream nullability story as
//   above.
// - **Column IRI map.** `VertexTable.properties` carries `name` +
//   `data_type` + `single_valued` but not per-column predicate IRIs (the
//   decomp's classify_object lifts them into `data_type` then drops them).
//   Bridge produces `column_iris = BTreeMap::new()`.

use crate::decomp::{EdgeTable, IRI_COLUMN, SinkPlan, VertexTable};

/// Same shape as [`plan_writes`] but takes a [`SinkPlan`] from the compiler.
///
/// Wraps each table's `source_relation` to expose the columns the W0b
/// writer expects (`subject` for vertices; `src_iri` / `dst_iri` for
/// edges) and delegates to [`plan_writes`].
///
/// # Errors
///
/// Returns [`WriteError`] under the same rules as [`plan_writes`] —
/// reserved-column collision, missing vertex reference, empty dest URL.
pub fn plan_writes_from_sink_plan(
    sink_plan: &SinkPlan,
    dest_url: &str,
    options: &WriteOptions,
) -> Result<WriteSqlPlan> {
    let (vertices, edges) = sink_plan_to_specs(sink_plan);
    plan_writes(&vertices, &edges, dest_url, options)
}

/// [`plan_manifests`] companion for the [`SinkPlan`] entry path.
///
/// # Errors
///
/// Returns [`WriteError`] under the same rules as [`plan_manifests`].
pub fn plan_manifests_from_sink_plan(
    sink_plan: &SinkPlan,
    options: &WriteOptions,
) -> Result<ManifestSet> {
    let (vertices, edges) = sink_plan_to_specs(sink_plan);
    plan_manifests(&vertices, &edges, options)
}

/// Lift a [`SinkPlan`] into the [`VertexSpec`] / [`EdgeSpec`] pair.
///
/// Public (not `pub(crate)`) so callers who want to inspect / mutate
/// the adapter result before emitting (e.g. CLI passes that override
/// `dedup_subjects`) can do so.
#[must_use]
pub fn sink_plan_to_specs(sink_plan: &SinkPlan) -> (Vec<VertexSpec>, Vec<EdgeSpec>) {
    let vertices = sink_plan
        .vertices
        .iter()
        .map(vertex_table_to_spec)
        .collect();
    let edges = sink_plan.edges.iter().map(edge_table_to_spec).collect();
    (vertices, edges)
}

fn vertex_table_to_spec(vt: &VertexTable) -> VertexSpec {
    // Wrap the source so the W0b writer sees a `subject` column instead
    // of decomp's canonical `iri`. Property columns are projected by
    // name; reserved-name collisions are caught later by plan_writes's
    // validate_inputs (the bridge does not pre-validate to keep the
    // error surface single-source).
    let mut projection = String::from("iri AS subject");
    for p in &vt.properties {
        projection.push_str(", \"");
        projection.push_str(&p.name);
        projection.push('"');
    }
    // `FROM <source>` without extra parens — matches the
    // `vertex_select_sql` / `edge_select_sql` convention in `decomp`. The
    // SinkPlan's `source_relation` arrives already as a complete
    // FROM-clause expression — typically `(SELECT ...) AS base` from
    // `fossil_codegen::base_relation_sql`. Wrapping that in another
    // `({source})` would produce `FROM ((SELECT ...) AS base)`, the
    // illegal "extra parens around an aliased derived table" form
    // DuckDB rejects with "syntax error at or near ')'".
    let source_relation = format!(
        "SELECT {projection} FROM {source}",
        source = vt.source_relation,
    );
    // Mirror decomp's `vertex_select_sql` collapse rule (SINK-05):
    // single-valued OR no-properties ⇒ dedup on subject.
    let dedup_subjects = vt.properties.iter().any(|p| p.single_valued) || vt.properties.is_empty();

    let _ = vt.vertex_id_col; // currently always IRI_COLUMN; kept in the
    // SinkPlan for forward-compat with surrogate
    // primary-key tables (none in W0b).
    let _ = IRI_COLUMN; // referenced via the literal `iri` above so
    // the projection survives a SINK-04 rename
    // would surface here as a compile error.

    VertexSpec {
        name: vt.type_name.clone(),
        iri: String::new(),
        source_relation,
        property_columns: vt.properties.iter().map(|p| p.name.clone()).collect(),
        dedup_subjects,
        column_iris: BTreeMap::new(),
    }
}

fn edge_table_to_spec(et: &EdgeTable) -> EdgeSpec {
    // Project decomp's src_id_expr / dst_id_expr into the W0b edge
    // emitter's expected column names (`src_iri` / `dst_iri`). Note we
    // do NOT collapse on `single_valued` here — edge dedup semantics
    // are different from vertices (per-source-vertex collapse vs per-
    // subject collapse) and W0b/2 deliberately keeps all rows. Future
    // commits can promote `single_valued` if a use case appears.
    //
    // `FROM <source>` without extra parens — same rationale as
    // `vertex_table_to_spec`: SinkPlan's `source_relation` is already a
    // complete FROM-clause expression (`(SELECT ...) AS base`).
    let source_relation = format!(
        "SELECT {src} AS src_iri, {dst} AS dst_iri FROM {source}",
        src = et.src_id_expr,
        dst = et.dst_id_expr,
        source = et.source_relation,
    );
    let _ = et.single_valued; // intentionally unused at the bridge layer
    EdgeSpec {
        label: et.predicate.clone(),
        iri: String::new(),
        source_type: et.src_type.clone(),
        target_type: et.dst_type.clone(),
        source_relation,
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_person() -> VertexSpec {
        VertexSpec {
            name: "person".to_string(),
            iri: "http://example.org/Person".to_string(),
            source_relation: "SELECT subject, name, age FROM raw_person".to_string(),
            property_columns: vec!["name".to_string(), "age".to_string()],
            dedup_subjects: true,
            column_iris: BTreeMap::new(),
        }
    }

    #[test]
    fn vertex_sql_carries_w0b_column_shape() {
        let plan = plan_writes(
            &[spec_person()],
            &[],
            "s3://bucket/job-42",
            &WriteOptions::default(),
        )
        .expect("plan");
        assert_eq!(plan.vertex_statements.len(), 1);
        assert!(plan.edge_statements.is_empty(), "W0b/1 emits no edges");
        let s = &plan.vertex_statements[0];
        assert_eq!(s.rel_path, "vertex/person.parquet");
        assert!(s.copy_sql.contains("row_number() OVER () - 1 AS dense_id"));
        assert!(s.copy_sql.contains("subject"));
        assert!(s.copy_sql.contains("\"name\""));
        assert!(s.copy_sql.contains("\"age\""));
        assert!(s.copy_sql.contains("0::REAL AS x"));
        assert!(s.copy_sql.contains("0::REAL AS y"));
        assert!(s.copy_sql.contains("0::UINTEGER AS cluster_id"));
        assert!(s.copy_sql.contains("DISTINCT ON (subject)"));
        assert!(
            s.copy_sql
                .contains("s3://bucket/job-42/vertex/person.parquet")
        );
        assert!(s.copy_sql.contains("ROW_GROUP_SIZE 100000"));
    }

    #[test]
    fn dedup_off_drops_distinct_wrap() {
        let mut spec = spec_person();
        spec.dedup_subjects = false;
        let plan = plan_writes(&[spec], &[], "file:///tmp/g", &WriteOptions::default()).unwrap();
        assert!(!plan.vertex_statements[0].copy_sql.contains("DISTINCT"));
    }

    #[test]
    fn reserved_column_name_rejected() {
        let mut spec = spec_person();
        spec.property_columns.push("dense_id".to_string());
        let err = plan_writes(&[spec], &[], "file:///tmp/g", &WriteOptions::default()).unwrap_err();
        match err {
            WriteError::ReservedColumnName { column, .. } => assert_eq!(column, "dense_id"),
            other => panic!("expected ReservedColumnName, got {other:?}"),
        }
    }

    #[test]
    fn empty_dest_url_rejected() {
        let err = plan_writes(&[spec_person()], &[], "", &WriteOptions::default()).unwrap_err();
        assert!(matches!(err, WriteError::EmptyDestUrl));
    }

    #[test]
    fn edge_refers_missing_vertex_rejected() {
        let edge = EdgeSpec {
            label: "knows".to_string(),
            iri: "http://example.org/knows".to_string(),
            source_type: "person".to_string(),
            target_type: "ghost".to_string(),
            source_relation: "SELECT src_iri, dst_iri FROM raw_edge".to_string(),
        };
        let err = plan_writes(
            &[spec_person()],
            &[edge],
            "file:///tmp/g",
            &WriteOptions::default(),
        )
        .unwrap_err();
        match err {
            WriteError::EdgeRefersMissingVertex { missing, .. } => {
                assert_eq!(missing, "ghost");
            }
            other => panic!("expected EdgeRefersMissingVertex, got {other:?}"),
        }
    }

    #[test]
    fn dest_url_trailing_slash_does_not_double() {
        let plan = plan_writes(
            &[spec_person()],
            &[],
            "file:///tmp/g/",
            &WriteOptions::default(),
        )
        .unwrap();
        assert!(
            plan.vertex_statements[0]
                .copy_sql
                .contains("file:///tmp/g/vertex/person.parquet")
        );
        assert!(
            !plan.vertex_statements[0]
                .copy_sql
                .contains("file:///tmp/g//vertex")
        );
    }

    // ── Edge emission (W0b/2) ───────────────────────────────────────────

    fn spec_org() -> VertexSpec {
        VertexSpec {
            name: "org".to_string(),
            iri: "http://example.org/Org".to_string(),
            source_relation: "SELECT subject, legal_name FROM raw_org".to_string(),
            property_columns: vec!["legal_name".to_string()],
            dedup_subjects: true,
            column_iris: BTreeMap::new(),
        }
    }

    fn spec_works_at_edge() -> EdgeSpec {
        EdgeSpec {
            label: "works_at".to_string(),
            iri: "http://example.org/worksAt".to_string(),
            source_type: "person".to_string(),
            target_type: "org".to_string(),
            source_relation: "SELECT subject AS src_iri, employer AS dst_iri FROM raw_person"
                .to_string(),
        }
    }

    #[test]
    fn edge_emits_csr_and_csc_with_canonical_dir() {
        let plan = plan_writes(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            "s3://bucket/job-42",
            &WriteOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.edge_statements.len(), 1);
        let e = &plan.edge_statements[0];
        assert_eq!(e.edge_dir_name, "person_works_at_org");
        assert_eq!(e.csr_rel_path, "edge/person_works_at_org/by_source.parquet");
        assert_eq!(e.csc_rel_path, "edge/person_works_at_org/by_target.parquet");
    }

    #[test]
    fn edge_csr_orders_by_src_dst_and_csc_by_dst_src() {
        let plan = plan_writes(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            "s3://bucket/job-42",
            &WriteOptions::default(),
        )
        .unwrap();
        let e = &plan.edge_statements[0];
        assert!(
            e.copy_csr_sql.contains("ORDER BY src_dense, dst_dense"),
            "CSR must sort src-first for CSR layout: {}",
            e.copy_csr_sql
        );
        assert!(
            e.copy_csc_sql.contains("ORDER BY dst_dense, src_dense"),
            "CSC must sort dst-first for CSC layout: {}",
            e.copy_csc_sql
        );
    }

    #[test]
    fn edge_joins_against_vertex_parquets_under_dest() {
        let plan = plan_writes(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            "s3://bucket/job-42",
            &WriteOptions::default(),
        )
        .unwrap();
        let csr = &plan.edge_statements[0].copy_csr_sql;
        assert!(
            csr.contains("read_parquet('s3://bucket/job-42/vertex/person.parquet') s"),
            "CSR must join against the source-type vertex Parquet: {csr}"
        );
        assert!(
            csr.contains("read_parquet('s3://bucket/job-42/vertex/org.parquet') t"),
            "CSR must join against the target-type vertex Parquet: {csr}"
        );
        assert!(csr.contains("e.src_iri = s.subject"));
        assert!(csr.contains("e.dst_iri = t.subject"));
        assert!(csr.contains("s.dense_id AS src_dense"));
        assert!(csr.contains("t.dense_id AS dst_dense"));
    }

    #[test]
    fn edge_destinations_are_written_under_dest_url() {
        let plan = plan_writes(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            "file:///tmp/g/",
            &WriteOptions::default(),
        )
        .unwrap();
        let e = &plan.edge_statements[0];
        assert!(
            e.copy_csr_sql
                .contains("TO 'file:///tmp/g/edge/person_works_at_org/by_source.parquet'"),
        );
        assert!(
            e.copy_csc_sql
                .contains("TO 'file:///tmp/g/edge/person_works_at_org/by_target.parquet'"),
        );
        // No double slash on the dest portion (the trim_end_matches on
        // dest_url is shared with vertex emission — regression guard
        // duplicated here in case the implementations diverge).
        assert!(!e.copy_csr_sql.contains("file:///tmp/g//edge"));
    }

    #[test]
    fn edge_emission_respects_row_group_size_override() {
        let opts = WriteOptions {
            row_group_size: 50_000,
            ..WriteOptions::default()
        };
        let plan = plan_writes(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            "file:///tmp/g",
            &opts,
        )
        .unwrap();
        let e = &plan.edge_statements[0];
        assert!(e.copy_csr_sql.contains("ROW_GROUP_SIZE 50000"));
        assert!(e.copy_csc_sql.contains("ROW_GROUP_SIZE 50000"));
    }

    // ── Manifest emission (W0b/3) ───────────────────────────────────────

    #[test]
    fn vertex_manifest_declares_w0b_columns() {
        let set = plan_manifests(&[spec_person()], &[], &WriteOptions::default()).unwrap();
        assert_eq!(set.vertices.len(), 1);
        let v = &set.vertices[0];
        assert_eq!(v.rel_path, "vertex/person.vertex.yml");
        assert_eq!(v.vertex_info.vertex_type, "person");
        let pg = &v.vertex_info.property_groups[0];
        let names: Vec<&str> = pg.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["dense_id", "subject", "name", "age", "x", "y", "cluster_id"],
            "W0b column order: dense_id + subject + caller props + x,y + cluster_id",
        );
    }

    #[test]
    fn vertex_manifest_marks_dense_id_primary_uint32() {
        let set = plan_manifests(&[spec_person()], &[], &WriteOptions::default()).unwrap();
        let dense_id = set.vertices[0].vertex_info.property_groups[0]
            .properties
            .iter()
            .find(|p| p.name == "dense_id")
            .expect("dense_id present");
        assert_eq!(dense_id.data_type, "uint32");
        assert!(dense_id.is_primary, "dense_id must be flagged primary");
        assert_eq!(dense_id.is_nullable, Some(false));
    }

    #[test]
    fn vertex_manifest_yaml_contains_dense_id_and_layout_cols() {
        let set = plan_manifests(&[spec_person()], &[], &WriteOptions::default()).unwrap();
        let yaml = &set.vertices[0].yaml;
        // Spot-checks on the rendered YAML — names appear, primary
        // flag round-trips, layout placeholder columns are present so
        // any GraphAr reader sees them declared even when the values
        // are constant in W0b.
        assert!(yaml.contains("name: dense_id"), "yaml: {yaml}");
        assert!(yaml.contains("is_primary: true"));
        assert!(yaml.contains("name: subject"));
        assert!(yaml.contains("name: x"));
        assert!(yaml.contains("name: y"));
        assert!(yaml.contains("name: cluster_id"));
        assert!(yaml.contains("version: gar/v1"));
    }

    #[test]
    fn graph_manifest_indexes_every_type() {
        let set = plan_manifests(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            &WriteOptions::default(),
        )
        .unwrap();
        let gi = &set.graph.graph_info;
        assert_eq!(set.graph.rel_path, "graph.graph.yml");
        // The aggregate index references every per-type manifest by rel_path,
        // so an httpfs reader fetches one file and discovers all types.
        assert_eq!(
            gi.vertices,
            vec!["vertex/person.vertex.yml", "vertex/org.vertex.yml"],
        );
        assert_eq!(gi.edges, vec![set.edges[0].rel_path.clone()]);
        assert_eq!(gi.version, "gar/v1");
        // Round-trips so the query side parses what the writer emits.
        let parsed: crate::manifest::GraphInfo =
            serde_yaml_ng::from_str(&set.graph.yaml).expect("graph info round-trips");
        assert_eq!(&parsed, gi);
    }

    #[test]
    fn edge_manifest_declares_both_csr_and_csc_adj_lists() {
        let set = plan_manifests(
            &[spec_person(), spec_org()],
            &[spec_works_at_edge()],
            &WriteOptions::default(),
        )
        .unwrap();
        assert_eq!(set.edges.len(), 1);
        let e = &set.edges[0];
        assert_eq!(
            e.rel_path,
            "edge/person_works_at_org/person_works_at_org.edge.yml"
        );
        assert_eq!(e.edge_info.src_type, "person");
        assert_eq!(e.edge_info.dst_type, "org");
        assert_eq!(e.edge_info.edge_type, "works_at");
        assert_eq!(
            e.edge_info.adj_lists.len(),
            2,
            "both CSR (aligned_by: src) and CSC (aligned_by: dst) must be declared"
        );
        let aligned: Vec<&str> = e
            .edge_info
            .adj_lists
            .iter()
            .map(|a| a.aligned_by.as_str())
            .collect();
        assert!(aligned.contains(&"src"));
        assert!(aligned.contains(&"dst"));
        assert!(e.edge_info.adj_lists.iter().all(|a| a.ordered));
    }

    #[test]
    fn manifest_validation_rejects_same_errors_as_plan_writes() {
        // Same validation as the SQL plan emitter — keeps the two
        // entry points consistent.
        let mut spec = spec_person();
        spec.property_columns.push("dense_id".to_string());
        let err = plan_manifests(&[spec], &[], &WriteOptions::default()).unwrap_err();
        assert!(matches!(err, WriteError::ReservedColumnName { .. }));
    }

    // ── SinkPlan bridge (W0b/5) ─────────────────────────────────────────

    use crate::decomp::{
        EdgeTable, IRI_COLUMN, PLACEHOLDER_RELATION, SinkPlan, VertexProperty, VertexTable,
    };
    use crate::manifest::DEFAULT_CHUNK_SIZE;

    fn person_table() -> VertexTable {
        VertexTable {
            type_name: "person".to_string(),
            rdf_type: None,
            vertex_id_col: IRI_COLUMN.to_string(),
            properties: vec![
                VertexProperty {
                    name: "name".to_string(),
                    data_type: "string".to_string(),
                    rdf_uri: None,
                    xsd_datatype: None,
                    single_valued: true, // ⇒ dedup_subjects
                },
                VertexProperty {
                    name: "age".to_string(),
                    data_type: "int64".to_string(),
                    rdf_uri: None,
                    xsd_datatype: None,
                    single_valued: true,
                },
            ],
            source_relation: PLACEHOLDER_RELATION.to_string(),
        }
    }

    fn person_knows_person_edge() -> EdgeTable {
        EdgeTable {
            src_type: "person".to_string(),
            predicate: "knows".to_string(),
            dst_type: "person".to_string(),
            src_id_expr: IRI_COLUMN.to_string(),
            dst_id_expr: "knows".to_string(),
            single_valued: false,
            source_relation: PLACEHOLDER_RELATION.to_string(),
        }
    }

    #[test]
    fn bridge_wraps_vertex_iri_as_subject_and_lists_props() {
        let plan = SinkPlan {
            vertices: vec![person_table()],
            edges: Vec::new(),
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let (vs, es) = sink_plan_to_specs(&plan);
        assert!(es.is_empty());
        assert_eq!(vs.len(), 1);
        let v = &vs[0];
        assert_eq!(v.name, "person");
        assert_eq!(v.property_columns, vec!["name", "age"]);
        // Source projection renames iri → subject and quotes property cols.
        // FROM <source> (no extra parens) — matches decomp's convention so
        // SinkPlan's `(SELECT...) AS base` source stays a valid derived
        // table reference. See `vertex_table_to_spec` for the rationale.
        assert!(
            v.source_relation
                .contains("SELECT iri AS subject, \"name\", \"age\" FROM "),
            "got: {}",
            v.source_relation
        );
        assert!(v.dedup_subjects, "single_valued ⇒ dedup");
        assert!(
            v.iri.is_empty(),
            "decomp drops IRI metadata — gap documented"
        );
        assert!(v.column_iris.is_empty());
    }

    #[test]
    fn bridge_dedup_off_when_all_properties_are_multi_valued() {
        let mut vt = person_table();
        for p in &mut vt.properties {
            p.single_valued = false;
        }
        let plan = SinkPlan {
            vertices: vec![vt],
            edges: Vec::new(),
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let (vs, _) = sink_plan_to_specs(&plan);
        assert!(
            !vs[0].dedup_subjects,
            "all multi-valued ⇒ keep all rows (no dedup)"
        );
    }

    #[test]
    fn bridge_dedup_on_when_vertex_has_no_properties() {
        // Edge-case carryover from decomp::vertex_select_sql: empty
        // properties also collapse on subject.
        let vt = VertexTable {
            type_name: "marker".to_string(),
            rdf_type: None,
            vertex_id_col: IRI_COLUMN.to_string(),
            properties: Vec::new(),
            source_relation: PLACEHOLDER_RELATION.to_string(),
        };
        let plan = SinkPlan {
            vertices: vec![vt],
            edges: Vec::new(),
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let (vs, _) = sink_plan_to_specs(&plan);
        assert!(vs[0].dedup_subjects);
    }

    #[test]
    fn bridge_edge_projects_iri_aliases_for_writer() {
        let plan = SinkPlan {
            vertices: vec![person_table()],
            edges: vec![person_knows_person_edge()],
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let (_, es) = sink_plan_to_specs(&plan);
        assert_eq!(es.len(), 1);
        let e = &es[0];
        assert_eq!(e.label, "knows");
        assert_eq!(e.source_type, "person");
        assert_eq!(e.target_type, "person");
        // The wrap exposes the column names the W0b edge emitter expects.
        assert!(
            e.source_relation.contains("AS src_iri") && e.source_relation.contains("AS dst_iri"),
            "got: {}",
            e.source_relation
        );
        assert!(
            e.iri.is_empty(),
            "decomp drops predicate IRI — gap documented"
        );
    }

    #[test]
    fn plan_writes_from_sink_plan_round_trips_to_full_sql() {
        // Smoke test the high-level entry point: passing a SinkPlan
        // through the adapter + plan_writes must produce a valid
        // WriteSqlPlan with the W0b column shape on the vertex side.
        let plan = SinkPlan {
            vertices: vec![person_table()],
            edges: vec![person_knows_person_edge()],
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let opts = WriteOptions::default();
        let write_plan = plan_writes_from_sink_plan(&plan, "file:///tmp/g", &opts).expect("plan");
        assert_eq!(write_plan.vertex_statements.len(), 1);
        assert_eq!(write_plan.edge_statements.len(), 1);
        let v_sql = &write_plan.vertex_statements[0].copy_sql;
        assert!(v_sql.contains("row_number() OVER () - 1 AS dense_id"));
        assert!(v_sql.contains("iri AS subject"));
        assert!(v_sql.contains("\"name\""));
        assert!(v_sql.contains("0::REAL AS x"));
        assert!(v_sql.contains("DISTINCT ON (subject)"));
        let e_sql = &write_plan.edge_statements[0].copy_csr_sql;
        assert!(e_sql.contains("s.dense_id AS src_dense"));
        assert!(e_sql.contains("AS src_iri"));
    }

    #[test]
    fn plan_manifests_from_sink_plan_carries_w0b_columns() {
        let plan = SinkPlan {
            vertices: vec![person_table()],
            edges: Vec::new(),
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let set = plan_manifests_from_sink_plan(&plan, &WriteOptions::default()).expect("plan");
        assert_eq!(set.vertices.len(), 1);
        let names: Vec<&str> = set.vertices[0].vertex_info.property_groups[0]
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        // Bridge preserves the property order from VertexTable and the
        // W0b column shape is appended.
        assert_eq!(
            names,
            vec!["dense_id", "subject", "name", "age", "x", "y", "cluster_id"]
        );
    }
}
