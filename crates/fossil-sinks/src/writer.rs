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
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
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

    let vertex_statements = vertices
        .iter()
        .map(|spec| emit_vertex_statement(spec, dest_url, options))
        .collect();

    // W0b/2 will populate this. Reserving the surface here so the public
    // shape (and downstream tests like fossil-graph snapshot consumers)
    // doesn't shift on each sub-commit.
    let edge_statements = Vec::new();

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
}
