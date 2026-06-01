//! MIR → `DuckDB` SQL lowering — Phase 1 hand-formatted templates.
//!
//! Per RESEARCH.md Pitfall 6, do **not** round-trip the full `DuckDB`
//! `COPY (...) TO '...' (FORMAT PARQUET)` statement through `sqlparser` — the
//! 0.59 release does not preserve the parenthesised options form on
//! `Statement::Copy::to_string()`. The hybrid pattern documented in the
//! research is: build the inner `SELECT` via sqlparser AST, then wrap it in
//! a hand-formatted COPY template.
//!
//! For Phase 1 the inner `SELECT` is trivially simple (3 select-list items,
//! 1 `FROM` clause), so even the inner-SELECT path is hand-formatted. The
//! `sqlparser` workspace dep is wired in [`Cargo.toml`] for Phase 4 readiness
//! when the 30-mapping corpus warrants AST construction; Phase 1 deliberately
//! exercises zero `sqlparser` API surface (de-risk).
//!
//! # Public Salsa query signature (Phase 2-9 contract — locked)
//!
//! ```ignore
//! #[salsa::tracked]
//! pub struct SqlPlan<'db> {
//!     #[returns(ref)] pub sql: String,
//!     #[returns(ref)] pub manifest_yaml: String,
//! }
//!
//! #[salsa::tracked]
//! pub fn codegen_sql<'db>(
//!     db: &'db dyn fossil_base::Db,
//!     mapping: fossil_hir::MappingLoc<'db>,
//! ) -> SqlPlan<'db>;
//! ```

use std::fmt::Write as _;
use std::sync::LazyLock;

use fossil_descriptors_output::OutputDescriptorKind;
use fossil_hir::body::body;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;
use fossil_hir::lower::lower_to_hir;
use fossil_hir::{HirExpr, MappingLoc, Primitive, PropertyKey, Ty, TyKind};
use fossil_mir::op::{AggFn, CmpOp, JoinKind, SourceFormat};
use fossil_mir::{Expr, MirGraph, Op, lower_to_mir};
use fossil_registry::{FunctionRegistry, InlineForm, LoweringKind};
use fossil_sinks::decomp::{
    EdgeTable, IRI_COLUMN, SinkPlan, VertexProperty, VertexTable, edge_select_sql, local_name,
    vertex_edge_decomp_from_kind, vertex_select_sql,
};
use fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;

use crate::ast;
use crate::manifest::{flat_triple_manifest, manifest_yaml_for_plan};

/// The stdlib classification catalog (05-01), read by [`render_expr`]'s
/// [`Expr::Call`] arm to lower a scalar call to its `DuckDB` builtin / inline
/// SQL / native-UDF marker.
///
/// # No `Box<dyn>` in Salsa (ADR-0015 / CLAUDE.md hard rule)
///
/// [`render_expr`] runs INSIDE the [`codegen_graph`] `#[salsa::tracked]` query.
/// The registry it consults is this plain `&'static FunctionRegistry`, NEVER a
/// trait object fetched through `db.system()`. The v0.1 stdlib is
/// program-invariant (no federation — REG-01 deferred), so a process-lifetime
/// `LazyLock` static is correct and sidesteps the Salsa-interning hazard
/// entirely. Dispatch is over the [`LoweringKind`] enum, not a trait.
static STDLIB: LazyLock<FunctionRegistry> = LazyLock::new(FunctionRegistry::stdlib_default);

#[salsa::tracked]
pub struct SqlPlan<'db> {
    #[returns(ref)]
    pub sql: String,
    #[returns(ref)]
    pub manifest_yaml: String,
}

/// Lower one `MappingLoc` to a [`SqlPlan`] containing the `DuckDB` SQL script
/// (`CREATE VIEW` + `COPY`) and the `GraphAr` manifest YAML.
///
/// Thin wrapper over the [`codegen_graph`] test seam: lowers the mapping to its
/// [`MirGraph`] then defers all MIR-walking to `codegen_graph`. Keeping the walk
/// behind a `MirGraph`-keyed helper lets the `tests/codegen_ops.rs` suite drive
/// directly-constructed graphs for the source-unreachable operators (ADR-0009)
/// without needing a `.fossil` source.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn codegen_sql<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> SqlPlan<'db> {
    codegen_graph(db, lower_to_mir(db, mapping))
}

/// The descriptor-driven codegen seam (SC#4 option (b), SINK-01/03/04/05) — a
/// **plain-Rust outer wrapper** over the tracked [`codegen_graph`] (ADR-0019).
///
/// # Why plain Rust, not `#[salsa::tracked]` (the B2 resolution)
///
/// [`OutputDescriptorKind`] carries a `ShExDescriptor` (a `shex_ast::Schema` +
/// resolved `HashMap<String, ShapeBinding>` heap state) that does NOT satisfy
/// the `salsa::Update` / `Eq` / `Hash` / `Clone` bounds Salsa interning
/// requires. Threading it into a tracked-query key would force those bounds
/// (impossible) AND would add a per-mapping descriptor read — breaking
/// `MAX_PER_MAPPING_FAN_OUT = 1` (Pitfall 6). So this wrapper:
///
/// 1. calls the existing `#[salsa::tracked]` [`codegen_graph`] for the
///    type-checked / interned MIR work (UNCHANGED — stays the only tracked
///    query; the descriptor never enters its key);
/// 2. applies the descriptor-driven vertex/edge decomposition + chunked COPY
///    emission as a plain-Rust POST-PASS over the MIR `Op`s + the descriptor.
///
/// `AcceptAll` (the walking-skeleton case — `hello.fossil` has no `ShEx` target)
/// returns [`codegen_graph`]'s output **byte-identically** (the flat-triple
/// single COPY). A `ShEx` descriptor emits one chunked `COPY ... TO
/// '<prefix>/chunk{k}.parquet' (FORMAT PARQUET)` per vertex table and per edge
/// table (option (b)).
///
/// `row_count_for` is the plain-Rust row-count oracle the chunked-COPY emission
/// consults to decide how many chunk statements to emit per table (one per
/// `chunk_size` rows). It is given the inner SELECT body of a table and returns
/// that table's row count. For the descriptor-less / unknown case the caller
/// passes a closure returning `None` and a single chunk is emitted (the chunk
/// count is purely an emission concern — NOT a tracked query, so a runtime
/// `SELECT count(*)` is legitimate here, per ADR-0019).
///
/// Returns `(sql, manifest_yaml)` — the SQL script (CREATE VIEWs + the per-table
/// chunked COPYs) and the concatenated `GraphAr` v1.0.0 manifest YAML.
#[allow(clippy::elidable_lifetime_names)]
pub fn codegen_sql_with_descriptor<'db>(
    db: &'db dyn fossil_base::Db,
    mir: MirGraph<'db>,
    kind: &OutputDescriptorKind,
    chunk_size: u64,
    mut row_count_for: impl FnMut(&str) -> Option<u64>,
) -> (String, String) {
    // (a) The tracked MIR work. For AcceptAll the flat-triple COPY this emits is
    //     the byte-identical walking-skeleton output — we return it verbatim.
    let tracked = codegen_graph(db, mir);
    if matches!(kind, OutputDescriptorKind::AcceptAll(_)) {
        return (tracked.sql(db).clone(), tracked.manifest_yaml(db).clone());
    }

    // (b) ShEx descriptor → plain-Rust post-pass over the MIR ops.
    let ops = mir.ops(db);

    // The CREATE VIEW prelude (every Source op) is shared by all per-table
    // COPYs — emit it once, verbatim from the tracked walk's leading views.
    let mut sql = String::new();
    for op in ops {
        if let Op::Source {
            uri,
            format,
            row_type: _,
        } = op
        {
            let view_name = derive_view_name(uri);
            let reader = source_reader(*format, uri);
            writeln!(sql, "CREATE VIEW {view_name} AS\nSELECT * FROM {reader};")
                .expect("writing to a String never fails");
        }
    }

    // The base relation the vertex/edge SELECTs read FROM: project each
    // TripleEmit's resolved subject as `iri` and each object as its predicate's
    // local name (the columns the decomposition references). Emits sharing a
    // FROM relation collapse into one base subquery (the v0.1 single-source
    // mapping; multi-source joins are Phase-6 territory). This is the seam the
    // Phase-6 Db-wiring lights up unchanged — it supplies the same base relation
    // from the resolved descriptor instead of the fixture.
    let base = base_relation_sql(db, ops);

    // Decompose under the descriptor (option (b) — the descriptor is an
    // argument, never read via Db::system()).
    let plan = vertex_edge_decomp_from_kind(kind, &base, chunk_size);

    for v in &plan.vertices {
        // The inner vertex SELECT projects the id column as `id` (SINK-04), so
        // the chunk row_number() window orders by the OUTPUT alias `id`, not the
        // source `vertex_id_col`.
        let _ = &v.vertex_id_col;
        emit_chunked_copy(
            &mut sql,
            &vertex_select_sql(v),
            &vertex_copy_prefix(v),
            chunk_size,
            "id",
            &mut row_count_for,
        );
    }
    for e in &plan.edges {
        emit_chunked_copy(
            &mut sql,
            &edge_select_sql(e),
            &edge_copy_prefix(e),
            chunk_size,
            "src_id",
            &mut row_count_for,
        );
    }

    let manifest = manifest_yaml_for_plan(&plan);
    (sql, manifest)
}

/// Decompose a mapping's `MirGraph` under a target descriptor and
/// expose the two pieces the W0b writer path needs.
///
/// Returns `(prelude_sql, sink_plan)`:
///
/// - `prelude_sql` is the `CREATE VIEW ...` statements that wire every
///   `Op::Source` URI into a `DuckDB` view. The W0b runtime executes
///   these BEFORE running the COPY plan returned by
///   `fossil_sinks::writer::plan_writes_from_sink_plan` — those COPY
///   statements reference the views by name.
/// - `sink_plan` is the per-shape `VertexTable` + per-predicate
///   `EdgeTable` decomposition. Its `source_relation` field is the
///   base relation SQL (same as the existing
///   [`codegen_sql_with_descriptor`] flat-emission path uses).
///
/// `AcceptAll` callers (no `ShEx` target) get a [`SinkPlan`] with a
/// single flat-triple passthrough vertex per
/// [`fossil_sinks::decomp::vertex_edge_decomp_from_kind`] — usable but
/// not the typical W0b-shape case; the existing
/// [`codegen_sql_with_descriptor`] flat path remains the
/// recommended entry for that case to preserve the
/// walking-skeleton invariant byte-for-byte.
///
/// This is the W0b/6 seam: it splits `codegen_sql_with_descriptor`'s
/// monolithic `(sql, manifest)` return into two independently
/// composable pieces so the W0b writer can build its own SQL on top of
/// the same decomposition.
#[allow(clippy::elidable_lifetime_names)]
pub fn decompose_for_writer<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    mir: MirGraph<'db>,
    kind: &OutputDescriptorKind,
    chunk_size: u64,
) -> (String, SinkPlan) {
    let ops = mir.ops(db);

    // Same prelude block as `codegen_sql_with_descriptor` — extracted to
    // a sibling function to keep the two entry points sharing one source
    // of truth for the CREATE VIEW shape.
    let mut prelude = String::new();
    for op in ops {
        if let Op::Source {
            uri,
            format,
            row_type: _,
        } = op
        {
            let view_name = derive_view_name(uri);
            let reader = source_reader(*format, uri);
            writeln!(
                prelude,
                "CREATE VIEW {view_name} AS\nSELECT * FROM {reader};"
            )
            .expect("writing to a String never fails");
        }
    }

    let base = base_relation_sql(db, ops);
    // No explicit output shape (`AcceptAll`): instead of the flat `_triples`
    // passthrough, SYNTHESISE the vertex decomposition from the typed mapping —
    // the mapping already declares the output (subject type + predicates +
    // forward-propagated datatypes). The shape "lives in fossil"; the host
    // authors no ShEx. An explicit `ShEx` descriptor still wins when supplied.
    let plan = match kind {
        OutputDescriptorKind::AcceptAll(_) => {
            synthesize_sink_plan(db, mapping, &base, chunk_size)
        }
        OutputDescriptorKind::ShEx(_) => vertex_edge_decomp_from_kind(kind, &base, chunk_size),
    };
    (prelude, plan)
}

/// Map a Fossil [`Primitive`] to its `GraphAr` data-type spelling — the same
/// vocabulary [`fossil_sinks::manifest::data_type_name`] emits.
const fn primitive_to_graphar(p: Primitive) -> &'static str {
    match p {
        Primitive::Integer => "int64",
        Primitive::Float => "double",
        Primitive::Bool => "bool",
        Primitive::Date => "date",
        Primitive::DateTime => "timestamp",
        Primitive::Time => "time",
        // String / AnyURI / GYear have no narrower GraphAr spelling.
        Primitive::String | Primitive::AnyURI | Primitive::GYear => "string",
    }
}

/// Peel `Optional`/`Seq` wrappers to the inner [`Primitive`], if any.
fn inner_primitive<'db>(db: &'db dyn fossil_base::Db, ty: Ty<'db>) -> Option<Primitive> {
    match ty.kind(db) {
        TyKind::Primitive(p) => Some(*p),
        TyKind::Optional(inner) | TyKind::Seq(inner) => inner_primitive(db, *inner),
        _ => None,
    }
}

/// Synthesise a single-vertex [`SinkPlan`] from a typed mapping when no explicit
/// output shape was supplied (`AcceptAll`). Mirrors [`lower_to_mir`]'s
/// header/body/source-row reads (so it never widens the per-mapping fan-out)
/// and reuses [`local_name`] for IRI localisation.
///
/// Phase A — literal properties only: a property whose RHS is a `FieldRef`
/// (column → its forward-propagated datatype) or a `StringLit` becomes a
/// [`VertexProperty`]. Template / IRI-valued objects are edges and are deferred
/// (Phase B); skipping them is safe — the base relation simply carries unused
/// columns. The result is a typed vertex table instead of the flat `_triples`
/// dump.
#[allow(clippy::elidable_lifetime_names)]
fn synthesize_sink_plan<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    source_relation: &str,
    chunk_size: u64,
) -> SinkPlan {
    let empty = SinkPlan {
        vertices: Vec::new(),
        edges: Vec::new(),
        chunk_size,
    };

    let file = mapping.file(db);
    let dm = def_map(db, file);
    let Some(idx) = dm.mappings(db).iter().position(|loc| *loc == mapping) else {
        return empty;
    };
    let hir = lower_to_hir(db, file);
    let Some(m) = hir.mappings(db).get(idx) else {
        return empty;
    };
    let type_name = local_name(m.shape_iri.as_str());

    // Source Record (forward-propagated field types) for datatype resolution.
    let source_row = typecheck_mapping(db, mapping)
        .ok()
        .and_then(|out| out.source_row(db));
    let field_datatype = |field: &str| -> &'static str {
        let Some(row) = source_row else { return "string" };
        let TyKind::Record(rec) = row.kind(db) else {
            return "string";
        };
        rec.fields(db)
            .iter()
            .find(|f| f.name == field)
            .and_then(|f| inner_primitive(db, f.ty))
            .map_or("string", primitive_to_graphar)
    };

    let mut properties = Vec::new();
    for prop in body(db, mapping).properties(db) {
        let PropertyKey::PrefixedName { iri } = &prop.key else {
            continue; // the `iri = ...` subject template is the vertex_id, not a property
        };
        let data_type = match &prop.value {
            HirExpr::FieldRef(field) => field_datatype(field.as_str()),
            HirExpr::StringLit(_) => "string",
            // Template / PrefixedName objects are IRIs → edges (Phase B).
            HirExpr::Template(_) | HirExpr::PrefixedName { .. } => continue,
        };
        properties.push(VertexProperty {
            name: local_name(iri.as_str()),
            data_type: data_type.to_string(),
            single_valued: true,
        });
    }

    SinkPlan {
        vertices: vec![VertexTable {
            type_name,
            vertex_id_col: IRI_COLUMN.to_string(),
            properties,
            source_relation: source_relation.to_string(),
        }],
        edges: Vec::new(),
        chunk_size,
    }
}

/// Convenience entry: lower a mapping then run [`codegen_sql_with_descriptor`].
/// The descriptor seam stays plain-Rust (NOT tracked) — see that function's doc.
#[allow(clippy::elidable_lifetime_names)]
pub fn codegen_sql_with_descriptor_for_mapping<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    kind: &OutputDescriptorKind,
    chunk_size: u64,
    row_count_for: impl FnMut(&str) -> Option<u64>,
) -> (String, String) {
    codegen_sql_with_descriptor(
        db,
        lower_to_mir(db, mapping),
        kind,
        chunk_size,
        row_count_for,
    )
}

/// Default rows-per-chunk for the descriptor seam.
///
/// Re-exported from `fossil_sinks::manifest::DEFAULT_CHUNK_SIZE` (1024) and
/// threaded as a param so a Phase-6 CLI `--chunk-size` flag can override it
/// without touching codegen.
pub const SINK_DEFAULT_CHUNK_SIZE: u64 = DEFAULT_CHUNK_SIZE;

/// Build the base relation subquery the vertex/edge SELECTs read `FROM`.
///
/// Each [`Op::TripleEmit`] contributes its resolved `subject` (projected as the
/// canonical `iri` column the decomposition uses verbatim — SINK-04) and its
/// `object` (projected as the predicate's local name — the column the vertex
/// property / edge dst references). All emits over a single source relation
/// collapse into one `SELECT <subject> AS iri, <obj0> AS <p0>, ... FROM <rel>`.
///
/// This mirrors the tracked walk's emit buffering but projects into the
/// decomposition's column convention rather than flat `(subject, predicate,
/// object)` triples. For the v0.1 single-source mapping all emits share one
/// FROM; the result is wrapped as `(<select>) AS base`.
fn base_relation_sql<'db>(db: &'db dyn fossil_base::Db, ops: &[Op<'db>]) -> String {
    // Replay the tracked walk's rel_ref / qualifier / extend buffering enough to
    // resolve each TripleEmit's subject/object exactly as codegen_graph does, so
    // the base relation columns match the flat-COPY's subject/object SQL.
    let mut rel_ref: Vec<Option<String>> = vec![None; ops.len()];
    let mut qualifier: Vec<Option<String>> = vec![None; ops.len()];
    let mut collapse_from: Vec<Option<String>> = vec![None; ops.len()];
    let mut collapse_qual: Vec<Option<String>> = vec![None; ops.len()];
    let mut extends: Vec<(String, String)> = Vec::new();
    // (iri_expr, predicate_local, object_expr, from)
    let mut emits: Vec<(String, String, String, String)> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op {
            Op::Source { uri, .. } => {
                let view = derive_view_name(uri);
                rel_ref[idx] = Some(view.clone());
                qualifier[idx] = Some(view);
            }
            Op::Extend { input, field, expr } => {
                let input_qual = input_qualifier(&qualifier, *input);
                let from = input_relation(&rel_ref, *input);
                let expr_sql = render_expr(expr, &input_qual);
                extends.push((field.to_string(), expr_sql.clone()));
                let body = ast::select_extend(&from, field, &expr_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
                collapse_from[idx] = Some(from);
                collapse_qual[idx] = Some(input_qual);
            }
            Op::TripleEmit {
                input,
                subject,
                predicate,
                object,
                graph: _,
            } => {
                let from = collapse_from
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_relation(&rel_ref, *input));
                let qual = collapse_qual
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_qualifier(&qualifier, *input));
                let subject_expr = render_emit_operand(subject, &extends, &qual);
                let object_expr = render_emit_operand(object, &extends, &qual);
                let pred_local = predicate_local_name(predicate);
                emits.push((subject_expr, pred_local, object_expr, from));
            }
            // Relational ops feeding the emit just thread their rel_ref; the
            // single-source v0.1 mapping does not exercise them on the sink
            // path, but keeping the threading honest means Phase-6 multi-op
            // mappings derive the right FROM.
            _ => {
                let _ = db;
            }
        }
    }

    if emits.is_empty() {
        return PLACEHOLDER_BASE.to_string();
    }
    // v0.1: all emits share one source relation. Project iri once + each
    // predicate-local object column.
    let from = emits[0].3.clone();
    let mut cols = format!("{} AS {}", emits[0].0, fossil_sinks::decomp::IRI_COLUMN);
    for (_, pred_local, object_expr, _) in &emits {
        write!(cols, ", {object_expr} AS {pred_local}").expect("writing to a String never fails");
    }
    format!("(SELECT {cols} FROM {from}) AS base")
}

/// Fallback base relation when a Sink has no upstream `TripleEmit` (a malformed
/// graph — never produced by `lower_to_mir`).
const PLACEHOLDER_BASE: &str = "(SELECT NULL AS iri) AS base";

/// The predicate's local name (segment after the last `/`, `#`, or `:`), the
/// column convention the decomposition uses for an object value.
fn predicate_local_name(predicate: &str) -> String {
    predicate
        .rsplit_once(['/', '#', ':'])
        .map_or(predicate, |(_, tail)| tail)
        .to_string()
}

/// The `GraphAr` chunk-file prefix for a vertex table: `vertex/<type>/`
/// (lowercased), matching the manifest `prefix` (05-05 / ADR-0016).
fn vertex_copy_prefix(v: &VertexTable) -> String {
    format!("vertex/{}", v.type_name.to_lowercase())
}

/// The `GraphAr` chunk-file prefix for an edge table:
/// `edge/<src>_<pred>_<dst>/` (lowercased).
fn edge_copy_prefix(e: &EdgeTable) -> String {
    format!(
        "edge/{}_{}_{}",
        e.src_type.to_lowercase(),
        e.predicate.to_lowercase(),
        e.dst_type.to_lowercase()
    )
}

/// Emit the chunked `COPY ... TO '<prefix>/chunk{k}.parquet' (FORMAT PARQUET)`
/// statements for one decomposed table (SINK-03 — larger-than-RAM).
///
/// Range-chunked multi-COPY (ADR-0019 chosen mechanism): the `inner` SELECT is
/// wrapped with `row_number() OVER (ORDER BY <order_col>) AS _rn`, and ONE COPY
/// is emitted per chunk range `WHERE _rn BETWEEN k*chunk_size+1 AND
/// (k+1)*chunk_size`, for `k in 0..ceil(row_count / chunk_size)`. The COPY
/// wrapper stays HAND-FORMATTED (Pitfall 1 — never round-trip through
/// sqlparser).
///
/// `row_count_for(inner)` supplies the table's row count so the correct N COPY
/// statements are emitted; `None` (unknown count) emits a single chunk
/// (`chunk0.parquet`). When `row_count > chunk_size` this emits N > 1 chunk
/// files — the SINK-03 multi-chunk proof.
fn emit_chunked_copy(
    sql: &mut String,
    inner: &str,
    prefix: &str,
    chunk_size: u64,
    order_col: &str,
    row_count_for: &mut impl FnMut(&str) -> Option<u64>,
) {
    let chunk_size = chunk_size.max(1);
    // Wrap the deterministic-ORDER BY inner SELECT with a stable row number so
    // each chunk range is a contiguous, reproducible slice (native↔WASM parity).
    let numbered =
        format!("SELECT *, row_number() OVER (ORDER BY {order_col}) AS _rn FROM ({inner}) AS _src");

    let n_chunks = match row_count_for(inner) {
        Some(rows) if rows > 0 => rows.div_ceil(chunk_size),
        // Unknown or empty: a single chunk file (still a valid GraphAr layout).
        _ => 1,
    };

    for k in 0..n_chunks {
        let lo = k * chunk_size + 1;
        let hi = (k + 1) * chunk_size;
        writeln!(
            sql,
            "COPY (\n    SELECT * EXCLUDE (_rn) FROM (\n{numbered}\n    ) AS _chunked\n    WHERE _rn BETWEEN {lo} AND {hi}\n) TO '{prefix}/chunk{k}.parquet' (FORMAT PARQUET);"
        )
        .expect("writing to a String never fails");
    }
}

/// The MIR → `DuckDB` SQL walk, keyed on a [`MirGraph`] rather than a mapping.
///
/// This is the codegen test seam (RESEARCH Code Examples / plan note): the 7
/// source-unreachable operators (`Project` / `Rename` / `Filter` / `Distinct` /
/// `Union` / `Empty`, + `Join` / `GroupBy` / `Aggregate` in plan 04-05) are
/// exercised by hand-building a `MirGraph` and calling
/// [`codegen_graph_for_test`] (the public test wrapper) — no surface syntax
/// required.
///
/// # Relation referencing
///
/// Each op produces a *relation reference* (`rel_ref[i]`) consumed by its
/// downstream ops:
/// - `Source` — a `CREATE VIEW` statement; its reference is the bare view name.
/// - the single-input SELECT ops (`Project` / `Rename` / `Filter` / `Distinct`
///   / `Empty`) and `Union` — build their SELECT body via the [`crate::ast`]
///   helpers; their reference is the body wrapped as `(<body>) AS step_<i>`
///   (nested subquery — chosen over a `WITH` CTE chain for self-containment and
///   snapshot stability; documented in ADR-0012).
/// - `Extend` — buffered, NOT emitted as a standalone SELECT, so its reference
///   is a *passthrough* of its input's reference. This is the byte-identical
///   `hello.fossil` path: the `iri` Extend collapses into the COPY's inner
///   SELECT exactly as in Phase 1.
///
/// `TripleEmit` / `Sink` keep the Phase 1 hand-wrapped COPY (Pitfall 1).
// `elidable_lifetime_names`: explicit 'db documents the Phase 2-9 contract.
// `too_many_lines`: the single op-dispatch match over the 12-variant Op enum is
// one coherent unit; splitting per-arm helpers would scatter the shared rel_ref
// / qualifier / extends / emit threading and hurt readability. Plan 04-05 adds
// the Join/GroupBy/Aggregate arms — revisit extraction then if it grows further.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names, clippy::too_many_lines)]
pub fn codegen_graph<'db>(db: &'db dyn fossil_base::Db, mir: MirGraph<'db>) -> SqlPlan<'db> {
    let ops = mir.ops(db);
    let mut sql = String::new();

    // The relation reference for each op index (the FROM-able SQL a downstream
    // op should read). `None` for terminal ops (TripleEmit/Sink) that produce
    // no consumable relation. For a Source this is the bare view name; for a
    // single-input SELECT op it is `(<body>) AS step_<idx>` (a nested subquery).
    let mut rel_ref: Vec<Option<String>> = vec![None; ops.len()];

    // The *column qualifier* (relation name) for each op index — what a
    // downstream `ColRef` should prefix with. For a Source it is the view name;
    // for a subquery op it is the subquery alias (`step_<idx>`). This is
    // distinct from `rel_ref` (the FROM text): a `ColRef` qualifies on the
    // alias, while the `FROM` clause carries the full `(<body>) AS step_<idx>`.
    let mut qualifier: Vec<Option<String>> = vec![None; ops.len()];

    // For a buffered Extend, the relation it reads FROM (so a consuming
    // TripleEmit collapses into a COPY over that same relation rather than a
    // redundant subquery — the byte-identical hello.fossil path) + the column
    // qualifier to use for that collapsed relation.
    let mut collapse_from: Vec<Option<String>> = vec![None; ops.len()];
    let mut collapse_qual: Vec<Option<String>> = vec![None; ops.len()];

    // Buffered Extends (field name → rendered expr SQL). An Extend feeding a
    // TripleEmit collapses into the COPY's inner SELECT (byte-identical
    // hello.fossil); it never becomes a standalone SELECT.
    let mut extends: Vec<(String, String)> = Vec::new();
    // (subject_expr, predicate_iri, object_expr, from_relation) — buffered by
    // each TripleEmit and consumed by Sink. A mapping with N TripleEmits
    // (multi-property, produced by 04-01's generalised lower_to_mir) collects N
    // entries; the Sink UNION-ALLs their triple-projections into one COPY. For
    // N = 1 the wrapper collapses to exactly the Phase-1 single-projection COPY
    // (byte-identical hello.fossil — guarded in the Sink arm).
    let mut emits: Vec<(String, String, String, String)> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op {
            Op::Source {
                uri,
                format,
                row_type: _,
            } => {
                let view_name = derive_view_name(uri);
                // STDL-06: the DuckDB table function is selected by the source
                // FORMAT. `read_csv_auto('{uri}', sample_size=-1)` is UNCHANGED
                // for Csv (byte-identical hello.fossil); Json/Parquet add their
                // own readers. All three run identically on native DuckDB and
                // DuckDB-WASM (SC#2 — codegen emits the SQL text; execution is
                // DuckDB's job).
                let reader = source_reader(*format, uri);
                writeln!(sql, "CREATE VIEW {view_name} AS\nSELECT * FROM {reader};")
                    .expect("writing to a String never fails");
                rel_ref[idx] = Some(view_name.clone());
                qualifier[idx] = Some(view_name);
            }
            Op::Extend { input, field, expr } => {
                // Buffer the computed field. Unqualified ColRefs in the expr
                // qualify on the input relation's column qualifier (the source
                // view name in the hello.fossil path).
                let input_qual = input_qualifier(&qualifier, *input);
                let from = input_relation(&rel_ref, *input);
                let expr_sql = render_expr(expr, &input_qual);
                extends.push((field.to_string(), expr_sql.clone()));
                // Dual exposure (ADR-0012):
                // - A downstream *TripleEmit* inlines the buffered expr into the
                //   COPY's inner SELECT (the byte-identical hello.fossil path —
                //   no standalone SELECT, no extra subquery).
                // - A downstream *relational* op (e.g. Extend→Filter) instead
                //   FROMs a standalone `SELECT *, <expr> AS "field"` subquery so
                //   the computed column is materialised. `select_extend` builds
                //   that body; the COPY collapse simply prefers the buffer.
                let body = ast::select_extend(&from, field, &expr_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
                // A buffered Extend that collapses into a COPY exposes its
                // input relation (FROM text) + qualifier so the emit FROMs the
                // source view directly and qualifies columns on it.
                collapse_from[idx] = Some(from);
                collapse_qual[idx] = Some(input_qual);
            }
            Op::Project { input, cols } => {
                let body = ast::select_cols_from(cols, &input_relation(&rel_ref, *input));
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Rename { input, old, new } => {
                let body = ast::select_rename(&input_relation(&rel_ref, *input), old, new);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Filter { input, pred } => {
                let pred_sql = render_expr(pred, &input_qualifier(&qualifier, *input));
                let body = ast::select_filter(&input_relation(&rel_ref, *input), &pred_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Distinct { input, by } => {
                let body = ast::select_distinct(&input_relation(&rel_ref, *input), by.as_deref());
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Union { left, right } => {
                let left_sql = ast::select_all_from(&input_relation(&rel_ref, *left));
                let right_sql = ast::select_all_from(&input_relation(&rel_ref, *right));
                let body = ast::union(&left_sql, &right_sql);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::Empty { schema } => {
                // ADR-0011 R9 target: a `WHERE false` shell over a notional
                // input view so the empty relation has the right column shape.
                let body = ast::select_empty(schema, "source");
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::TripleEmit {
                input,
                subject,
                predicate,
                object,
                graph: _,
            } => {
                // When the input is a buffered Extend, COPY over the relation
                // that Extend reads (collapse the computed column inline) — the
                // byte-identical hello.fossil path. Otherwise FROM the input op's
                // own relation (e.g. a Project/Filter subquery feeding emit).
                //
                // `from` is the FROM-clause text (view name OR `(<body>) AS
                // step_<i>` subquery); `qual` is the column qualifier (the view
                // name OR the subquery alias `step_<i>`) — distinct, since a
                // bare ColRef must prefix the alias, not the whole subquery.
                let from = collapse_from
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_relation(&rel_ref, *input));
                let qual = collapse_qual
                    .get(*input)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| input_qualifier(&qualifier, *input));
                // Resolve `subject` / `object`. A bare `ColRef { source: "",
                // column }` naming a buffered Extend (the shared `iri` column)
                // is substituted with that Extend's rendered SQL so the COPY
                // sees the resolved expression. Keeps hello.fossil
                // byte-identical with Phase 1.
                let subject_expr = render_emit_operand(subject, &extends, &qual);
                let object_expr = render_emit_operand(object, &extends, &qual);
                emits.push((subject_expr, predicate.to_string(), object_expr, from));
            }
            Op::Sink { input: _, sink: _ } => {
                // Generalised SinkOp/COPY: wrap the buffered TripleEmit
                // projection(s) into one `COPY (...) TO 'output.parquet'
                // (FORMAT PARQUET)`. For a single TripleEmit the inner SELECT is
                // the Phase-1 shape verbatim (byte-identical hello.fossil); for
                // N > 1 the N triple-projections are `UNION ALL`'d (flat-triple
                // GraphAr Phase-1 contract — Phase 5 owns vertex/edge
                // decomposition). The COPY stays HAND-WRAPPED (Pitfall 1 — never
                // round-trip through sqlparser).
                if let Some(inner) = sink_inner_select(&emits) {
                    writeln!(
                        sql,
                        "COPY (\n{inner}\n) TO 'output.parquet' (FORMAT PARQUET);"
                    )
                    .expect("writing to a String never fails");
                }
            }
            Op::Join {
                left,
                right,
                on,
                kind,
                left_name,
                right_name,
            } => {
                // Two-input join. Each side's relation reference becomes a
                // parenthesised subquery aliased on the binding name
                // (`left_name` / `right_name`) so columns disambiguate across
                // the union of the two schemas (operator-algebra.md §2.6).
                let left_sql = input_relation(&rel_ref, *left);
                let right_sql = input_relation(&rel_ref, *right);
                // The ON predicate's `ColRef`s already carry their binding name
                // (`orders` / `users`) as `source`; the `default_source` only
                // covers a bare (empty-source) ColRef, which a join ON should
                // never use. Pass the left binding name as the conventional
                // fallback.
                let on_sql = render_expr(on, left_name.as_str());
                let body = ast::select_join(
                    &left_sql,
                    left_name.as_str(),
                    &right_sql,
                    right_name.as_str(),
                    join_kind_sql(*kind),
                    &on_sql,
                );
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
            Op::GroupBy { input, keys } => {
                // A GroupBy establishes the grouping keys. If the immediately
                // following op is an Aggregate, that arm renders the paired
                // `SELECT <keys>, <aggs> ... GROUP BY <keys>` (operator-algebra
                // treats GroupBy + Aggregate as one SELECT). A GroupBy NOT
                // followed by an Aggregate is just the key projection.
                let aggregated = matches!(ops.get(idx + 1), Some(Op::Aggregate { input: agg_in, .. }) if *agg_in == idx);
                if !aggregated {
                    let body = ast::select_group_by(&input_relation(&rel_ref, *input), keys, &[]);
                    rel_ref[idx] = Some(subquery(&body, idx));
                    qualifier[idx] = Some(step_alias(idx));
                }
                // When the next op aggregates this GroupBy, defer to that arm —
                // leave this op's rel_ref unset; the Aggregate reads `keys` back
                // off this GroupBy node.
            }
            Op::Aggregate { input, aggs } => {
                // Pair with the upstream GroupBy (if any) so the keys + agg
                // exprs render into one `GROUP BY` SELECT. If `input` is not a
                // GroupBy, this is a bare aggregate (no keys → global aggregate).
                let (keys, group_input) = match ops.get(*input) {
                    Some(Op::GroupBy { input: gb_in, keys }) => (keys.clone(), *gb_in),
                    _ => (Vec::new(), *input),
                };
                let agg_exprs: Vec<String> = aggs
                    .iter()
                    .map(|spec| {
                        format!(
                            "{}({}) AS \"{}\"",
                            agg_fn_sql(spec.agg_fn),
                            spec.in_field,
                            spec.out_field,
                        )
                    })
                    .collect();
                let body =
                    ast::select_group_by(&input_relation(&rel_ref, group_input), &keys, &agg_exprs);
                rel_ref[idx] = Some(subquery(&body, idx));
                qualifier[idx] = Some(step_alias(idx));
            }
        }
    }

    SqlPlan::new(db, sql, flat_triple_manifest())
}

/// Test-only entry point into the codegen seam.
///
/// Drives [`codegen_graph`] with a hand-built [`MirGraph`] from the `tests/`
/// integration crate. The Salsa query itself stays `pub` for the wrapper, but
/// tests call through this stable name so the seam is explicit. Used to snapshot
/// the source-unreachable operators (ADR-0009).
#[allow(clippy::elidable_lifetime_names)]
pub fn codegen_graph_for_test<'db>(
    db: &'db dyn fossil_base::Db,
    mir: MirGraph<'db>,
) -> SqlPlan<'db> {
    codegen_graph(db, mir)
}

/// Build the inner SELECT of the terminal COPY from the buffered `TripleEmit`
/// projections.
///
/// - 0 emits → `None` (no COPY emitted — a Sink with no upstream `TripleEmit`).
/// - 1 emit → the Phase-1 single-projection SELECT verbatim (byte-identical
///   `hello.fossil`).
/// - N emits → the N triple-projections combined with `UNION ALL` (multi-
///   property mapping; flat-triple `GraphAr` Phase-1 contract).
///
/// The COPY wrapper itself stays hand-formatted in the caller (Pitfall 1).
fn sink_inner_select(emits: &[(String, String, String, String)]) -> Option<String> {
    if emits.is_empty() {
        return None;
    }
    let projection = |(subject, predicate, object, from): &(String, String, String, String)| {
        format!(
            "    SELECT\n        {subject} AS subject,\n        '{predicate}' AS predicate,\n        {object} AS object\n    FROM {from}"
        )
    };
    let inner = emits
        .iter()
        .map(projection)
        .collect::<Vec<_>>()
        .join("\n    UNION ALL\n");
    Some(inner)
}

/// Resolve op index `i`'s relation reference, falling back to the conventional
/// `"source"` view name when an op references an index that produced no
/// relation (a malformed graph — never in practice). `&rel_ref[i]` is the
/// view name (Source) or `(<body>) AS step_<i>` subquery (single-input ops).
fn input_relation(rel_ref: &[Option<String>], i: usize) -> String {
    rel_ref
        .get(i)
        .and_then(Clone::clone)
        .unwrap_or_else(|| "source".to_string())
}

/// Resolve op index `i`'s column qualifier (the relation name a downstream
/// `ColRef` prefixes), falling back to `"source"` for a malformed graph.
fn input_qualifier(qualifier: &[Option<String>], i: usize) -> String {
    qualifier
        .get(i)
        .and_then(Clone::clone)
        .unwrap_or_else(|| "source".to_string())
}

/// The subquery alias for op index `idx`: `step_<idx>`.
fn step_alias(idx: usize) -> String {
    format!("step_{idx}")
}

/// Wrap a SELECT body as a nested-subquery relation reference for downstream
/// ops: `(<body>) AS step_<idx>` (ADR-0012 — subquery over CTE chain).
fn subquery(body: &str, idx: usize) -> String {
    format!("({body}) AS {}", step_alias(idx))
}

/// Render a `TripleEmit` subject/object operand to SQL. A bare `ColRef` whose
/// `source` is empty and whose `column` names a buffered [`Op::Extend`] is
/// substituted with that Extend's rendered expression (so the shared `iri`
/// subject column resolves to its full template). Otherwise the operand renders
/// via [`render_expr`] qualified by the source view.
fn render_emit_operand(operand: &Expr<'_>, extends: &[(String, String)], view: &str) -> String {
    if let Expr::ColRef { source, column } = operand
        && source.is_empty()
        && let Some((_, expr_sql)) = extends.iter().find(|(name, _)| name == column.as_str())
    {
        return expr_sql.clone();
    }
    render_expr(operand, view)
}

/// Render an [`Expr`] tree to a `DuckDB` SQL fragment.
///
/// `default_source` is used as the qualifier for [`Expr::ColRef`] entries whose
/// `source` field is empty (the shared `iri` subject reference, and Phase 4's
/// multi-source joins where a `ColRef` may name a binding the codegen must
/// disambiguate).
///
/// `LitString` / `ColRef` / `Concat` render byte-identically with Phase 1. The
/// Phase 4..6 additions (`LitBool` / `Call` / `BinOp` / `Assert`) render as:
/// - `LitBool` → `TRUE` / `FALSE`
/// - `Call` → registry-driven (Phase 5 / STDL-02..05): the [`STDLIB`] catalog's
///   [`LoweringKind`] decides — a `DuckDB` `Builtin{duckdb_name}` call
///   (`clean.trim` → `trim(x)`), an [`InlineForm`] via [`render_inline`]
///   (`parse.integer` → `CAST(x AS BIGINT)`), or a `Udf{udf_name}` marker
///   (`clean.slug` → `fossil_slug(x)`) the native runtime resolves. Unknown /
///   `Plan`-kind names fall back to a defensive `func(arg, ...)` passthrough.
/// - `BinOp` → `lhs <op> rhs` ([`CmpOp`] → SQL operator)
/// - `Assert` → the SC#4 named runtime assertion
///   `CASE WHEN <guard> THEN <inner> ELSE error('fossil_assertion_<name>:line=<N>') END`
///   (P-CRIT-4 — never silent failure)
fn render_expr(expr: &Expr<'_>, default_source: &str) -> String {
    match expr {
        Expr::LitString(s) => format!("'{}'", s.replace('\'', "''")),
        Expr::LitBool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Expr::ColRef { source, column } => {
            let qualifier = if source.is_empty() {
                default_source
            } else {
                source.as_str()
            };
            format!("{qualifier}.{column}")
        }
        Expr::Concat(lhs, rhs) => {
            format!(
                "{} || {}",
                render_expr(lhs, default_source),
                render_expr(rhs, default_source),
            )
        }
        Expr::Call { func, args, ty: _ } => {
            // Render args recursively first. NOTE: this arm reads ONLY `func`
            // and `args` — never the `ty` field — so no `TyKind::Unknown` /
            // `InferenceId` debug text can ever be interpolated into the SQL
            // (STATE.md no-leak rule / RESEARCH Pitfall 5). The function is
            // looked up by name in the `&'static` STDLIB catalog (enum dispatch
            // over `LoweringKind`, no `Box<dyn>` across the Salsa boundary).
            let rendered: Vec<String> = args
                .iter()
                .map(|a| render_expr(a, default_source))
                .collect();
            match STDLIB.lookup(func).map(|e| &e.lowering) {
                // A DuckDB scalar/aggregate builtin called by name:
                // `clean.trim` → `trim(x)`, `anon.hash` → `sha256(x)`.
                Some(LoweringKind::Builtin { duckdb_name }) => {
                    format!("{duckdb_name}({})", rendered.join(", "))
                }
                // An inline SQL form (CAST / strptime / json_extract / literal /
                // ...): `parse.integer` → `CAST(x AS BIGINT)`.
                Some(LoweringKind::Inline(form)) => render_inline(form, &rendered),
                // A native Rust UDF: render the registered call name so the
                // native runtime (05-03) resolves it and the playground can
                // disable it: `clean.slug` → `fossil_slug(x)`.
                Some(LoweringKind::Udf { udf_name }) => {
                    format!("{udf_name}({})", rendered.join(", "))
                }
                // `Plan`-kind entries (the `seq/` operator family + `io/`
                // sources) are MIR ops, NOT scalar `Expr::Call`s — they are
                // surface-unreachable in v0.1 (ADR-0009) and never reach this
                // arm in practice. Render defensively as a passthrough rather
                // than panicking inside the tracked query.
                Some(LoweringKind::Plan(_)) => {
                    format!("{func}({})", rendered.join(", "))
                }
                // Unknown function: defensive passthrough (a malformed graph;
                // never produced by `lower_to_mir`, which only emits registered
                // calls).
                None => format!("{func}({})", rendered.join(", ")),
            }
        }
        Expr::BinOp {
            op,
            lhs,
            rhs,
            ty: _,
        } => {
            format!(
                "{} {} {}",
                render_expr(lhs, default_source),
                cmp_op_sql(*op),
                render_expr(rhs, default_source),
            )
        }
        // SC#4 / P-CRIT-4: a named runtime assertion. Where a static check
        // cannot be discharged (the IRI-template `${.field}` NULL case), emit
        // `CASE WHEN <guard> THEN <inner> ELSE error('fossil_assertion_<name>:line=<N>') END`
        // so the SQL fails LOUDLY at runtime with a named, line-located message
        // — never a silent malformed value. The `span_line` is resolved during
        // lowering (lower_to_mir, RESEARCH Pitfall 3) so this stays a pure
        // render. The guard is derived from the FIXED assertion name (no type
        // text is ever interpolated — Pitfall 5).
        Expr::Assert {
            name,
            span_line,
            inner,
        } => {
            let inner_sql = render_expr(inner, default_source);
            let guard = assert_guard_sql(name, &inner_sql);
            ast::render_assert(name, *span_line, &guard, &inner_sql)
        }
    }
}

/// Render an [`InlineForm`] (a `pure_sql` stdlib call that compiles to an inline
/// SQL expression, not a named function call) over its already-rendered
/// arguments.
///
/// Covers every [`InlineForm`] variant the 05-01 catalog defines:
/// - [`InlineForm::Cast`] → `CAST(<a0> AS <sql_type>)` (`parse.integer` →
///   `CAST(x AS BIGINT)`, `parse.float` → `DOUBLE`, `parse.decimal` →
///   `DECIMAL(38,18)`).
/// - [`InlineForm::Concat`] → the args joined with ` || ` (`core.triple`).
/// - [`InlineForm::LiteralStr`] → a fixed `'<value>'` literal, IGNORING args
///   (`anon.redact` → `'[REDACTED]'`). The value is single-quote-escaped.
/// - [`InlineForm::SplitPart`] → `split_part(<args>)` (`parse.csv_row`).
/// - [`InlineForm::JsonExtract`] → `json_extract(<a0>, <a1>)` (`parse.json`;
///   a single-arg call degrades to `json_extract(<a0>)` defensively).
/// - [`InlineForm::BlankNode`] → `'_:bnode_' || <a0>` (`core.blank`).
/// - [`InlineForm::RequireNonNull`] → `CASE WHEN <a0> IS NULL THEN
///   error('fossil_require_null') ELSE <a0> END` (`core.require`).
/// - [`InlineForm::Identity`] → the single argument verbatim (`core.iri` /
///   `core.literal` / `core.typed` / `core.lang` — the datatype/lang annotation
///   rides a side column, not this scalar expression).
///
/// Like the Call arm, this reads only the rendered argument strings and the
/// form's own fixed SQL text — never any type text — so it cannot leak
/// `TyKind::Unknown` / `InferenceId` into the SQL (RESEARCH Pitfall 5).
fn render_inline(form: &InlineForm, args: &[String]) -> String {
    // A safe argument accessor: a malformed (wrong-arity) call degrades to an
    // empty fragment rather than panicking inside the tracked query.
    let arg = |i: usize| args.get(i).map_or("", String::as_str);
    match form {
        InlineForm::Cast { sql_type } => format!("CAST({} AS {sql_type})", arg(0)),
        InlineForm::Concat => args.join(" || "),
        InlineForm::LiteralStr { value } => format!("'{}'", value.replace('\'', "''")),
        InlineForm::SplitPart => format!("split_part({})", args.join(", ")),
        InlineForm::JsonExtract => {
            if args.len() >= 2 {
                format!("json_extract({}, {})", arg(0), arg(1))
            } else {
                format!("json_extract({})", arg(0))
            }
        }
        InlineForm::BlankNode => format!("'_:bnode_' || {}", arg(0)),
        InlineForm::RequireNonNull => {
            let a0 = arg(0);
            format!("CASE WHEN {a0} IS NULL THEN error('fossil_require_null') ELSE {a0} END")
        }
        InlineForm::Identity => arg(0).to_string(),
    }
}

/// Derive the runtime guard SQL for a named assertion. The guard is the
/// statically-undischargeable condition that, when FALSE, trips the
/// `error(...)` branch.
///
/// - `iri_template_unbound` → `<inner> IS NOT NULL` (the `${.field}` value must
///   be present at runtime or the IRI is malformed — RESEARCH §"Named Runtime
///   Assertions" candidate 1).
/// - any other (future candidates 2/3 — required-but-Optional cardinality,
///   datatype-cast failures — plug in here) → a conservative `<inner> IS NOT
///   NULL` so the mechanism degrades safely rather than silently passing.
///
/// Matching on the FIXED `snake_case` name keeps the guard mapping explicit and
/// guarantees no type text leaks into the SQL (Pitfall 5).
fn assert_guard_sql(name: &str, inner_sql: &str) -> String {
    // Phase 4 ships only candidate 1 (`iri_template_unbound`). Future candidates
    // (required-but-Optional cardinality, datatype-cast failures) add arms here
    // with their own guards; for now every assertion's guard is "value present"
    // (`IS NOT NULL`), so a single uniform guard covers the catalogue. The
    // `name` is matched (not interpolated) so no type text can leak (Pitfall 5).
    debug_assert!(
        name == "iri_template_unbound",
        "unexpected assertion name {name:?} — add a guard arm in assert_guard_sql"
    );
    format!("{inner_sql} IS NOT NULL")
}

/// Map a [`JoinKind`] to its DuckDB-portable JOIN keyword
/// (operator-algebra.md §2.6).
///
/// Rendered as a string rather than a `sqlparser::ast::JoinOperator` because
/// sqlparser 0.59 Display's `JoinOperator::FullOuter` as `FULL JOIN`, dropping
/// the explicit `OUTER` the corpus snapshots want. Both `FULL JOIN` and
/// `FULL OUTER JOIN` are DuckDB-equivalent; we emit the explicit spelling for
/// every kind for uniformity. These keywords are DuckDB-portable (RESEARCH
/// Pitfall 6 — portable JOIN syntax only).
const fn join_kind_sql(kind: JoinKind) -> &'static str {
    match kind {
        JoinKind::Inner => "INNER JOIN",
        JoinKind::LeftOuter => "LEFT OUTER JOIN",
        JoinKind::RightOuter => "RIGHT OUTER JOIN",
        JoinKind::Full => "FULL OUTER JOIN",
    }
}

/// Map an [`AggFn`] to its `DuckDB` aggregate-function spelling
/// (operator-algebra.md §2.9).
const fn agg_fn_sql(agg: AggFn) -> &'static str {
    match agg {
        AggFn::Count => "COUNT",
        AggFn::Sum => "SUM",
        AggFn::Min => "MIN",
        AggFn::Max => "MAX",
        AggFn::Avg => "AVG",
    }
}

/// Map a [`CmpOp`] to its `DuckDB` SQL operator spelling.
const fn cmp_op_sql(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "<>",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::And => "AND",
        CmpOp::Or => "OR",
    }
}

/// Render the `DuckDB` table-function call that reads a source of `format` at
/// `uri` (STDL-06). This is the FROM-clause expression of the source view's
/// `SELECT * FROM <reader>`:
/// - [`SourceFormat::Csv`] → `read_csv_auto('{uri}', sample_size=-1)` (the
///   Phase-1 string verbatim — `hello.fossil` stays byte-identical).
/// - [`SourceFormat::Json`] → `read_json_auto('{uri}')`.
/// - [`SourceFormat::Parquet`] → `read_parquet('{uri}')`.
///
/// All three are DuckDB-portable table functions that behave identically on
/// native DuckDB and DuckDB-WASM (SC#2 — native↔WASM byte-identity is verified
/// end-to-end by plan 05-09's parity test).
#[allow(clippy::doc_markdown)] // read_csv_auto/read_json_auto/read_parquet are SQL fn names
fn source_reader(format: SourceFormat, uri: &str) -> String {
    match format {
        SourceFormat::Csv => format!("read_csv_auto('{uri}', sample_size=-1)"),
        SourceFormat::Json => format!("read_json_auto('{uri}')"),
        SourceFormat::Parquet => format!("read_parquet('{uri}')"),
    }
}

/// Derive a SQL view name from a source URI: `examples/users.csv` → `users`.
///
/// Phase 1 uses the filename stem. Phase 4 may add explicit binding-name
/// overrides when multiple sources share a stem (`users.csv` from two
/// directories, etc.).
fn derive_view_name(uri: &str) -> String {
    std::path::Path::new(uri)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("source")
        .to_string()
}
