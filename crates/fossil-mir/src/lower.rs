//! HIR → MIR lowering for the source-reachable operator subset.
//!
//! Consumes the per-mapping HEADER from [`fossil_hir::HirMapping`], the
//! per-mapping BODY from [`fossil_hir::body::body`] (separated per ADR-0005,
//! Plan 02-04), and the per-mapping TYPES from
//! [`fossil_hir::check::typecheck_mapping`] (Phase 3, CORE-04..07). Emits a
//! [`MirGraph`] of the shape:
//! `Source → Extend(iri = ...) → TripleEmit* → Sink(GraphAr)`.
//!
//! # Reachability (ADR-0009)
//!
//! `HirExpr` has only 4 leaf forms (`Template` / `FieldRef` / `StringLit` /
//! `PrefixedName`) — no surface pipeline / call / filter / join syntax. So only
//! 4 of the 11 [`Op`] variants are reachable from `.fossil` source: `Source`,
//! `Extend`, `TripleEmit`, `Sink`. This function lowers exactly those four. The
//! other 7 operators (`Project` / `Rename` / `Filter` / `Join` / `Union` /
//! `GroupBy` / `Aggregate` / `Distinct`) are exercised via direct `MirGraph`
//! construction in plans 04-04/04-05, NOT via source lowering. Surface pipeline
//! syntax is DEFERRED (see ADR-0009).
//!
//! # Phase 4 generalisations over the Phase 1 hardcodes
//!
//! - **Source row type** comes from [`fossil_hir::check::TypeckOutput`]'s
//!   `source_row` (CSVW-derived) when type-checking succeeds; otherwise it
//!   falls back to the Phase 1 `Record({id, name})` so codegen still produces
//!   output (walking-skeleton preserved — never panic).
//! - **Prefix expansion** uses the real per-file prefix table from
//!   [`fossil_hir::def_map`] instead of the hardcoded `ex:` →
//!   `https://example.org/`.
//! - **Multi-property mappings** emit one shared upstream `Extend(field="iri")`
//!   feeding N `TripleEmit`s (one per non-`iri` property), then one `Sink`.
//!
//! # Source URI + format (Phase 5 STDL-06)
//!
//! - **Source URI + format** are resolved from the mapping's source binding via
//!   [`fossil_hir::DefMap::lookup_source_call`] — the `io.csv` / `io.json` /
//!   `io.parquet` constructor name selects the [`SourceFormat`]; the
//!   constructor's first positional string is the URI. This replaces the
//!   Phase-1 hardcoded `examples/users.csv` / `Csv`. `def_map(db, file)` is
//!   already read here (file-keyed, structurally stable across body edits — see
//!   the barrier note below), so resolving the source call adds NO new
//!   per-mapping Salsa fan-out. A binding with no recognisable `io.*("...")`
//!   call (a malformed source) falls back to `examples/users.csv` / `Csv` so
//!   `lower_to_mir` never panics.
//!
//! # CRITICAL barrier rule (RESEARCH Pitfall 3)
//!
//! `lower_to_mir` may read `body(db, mapping)`, `typecheck_mapping(db, mapping)`
//! — all barrier-routed through `mapping_cst_node` per ADR-0005 + plan 02-07.
//! It MUST NOT add a `parse(db, file)` read in the per-mapping path (would
//! break `MAX_PER_MAPPING_FAN_OUT = 1`). `def_map(db, file)` is file-keyed and
//! structurally stable across body-only edits, so the `def_map` reads here do
//! not widen the per-mapping fan-out.
//!
//! # Public Salsa query signature (Phase 2-9 contract — locked)
//!
//! ```ignore
//! #[salsa::tracked]
//! pub fn lower_to_mir<'db>(
//!     db: &'db dyn fossil_base::Db,
//!     mapping: fossil_hir::MappingLoc<'db>,
//! ) -> MirGraph<'db>;
//! ```

use fossil_hir::body::{ExprId, HirBody, body, mapping_cst_node};
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::{DefMap, PrefixEntry, def_map};
use fossil_hir::lower::lower_to_hir;
use fossil_hir::spans::spans;
use fossil_hir::ty::RecordField;
use fossil_hir::{HirExpr, HirMapping, MappingLoc, Primitive, PropertyKey, Record, Ty, TyKind};
use smol_str::SmolStr;

use crate::graph::MirGraph;
use crate::op::{Expr, Op, SinkRef, SourceFormat};

/// Lower one [`fossil_hir::MappingLoc`] to a [`MirGraph`]:
/// `Source → Extend(iri) → TripleEmit* → Sink(GraphAr)`.
///
/// Generalised over the 4 source-reachable operators (ADR-0009). Emits one
/// shared `Extend(field="iri")` feeding N `TripleEmit`s (one per non-`iri`
/// property), then one `Sink`. The single-property `hello.fossil` produces the
/// same `Source → Extend → TripleEmit → Sink` sequence as Phase 1.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn lower_to_mir<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> MirGraph<'db> {
    let file = mapping.file(db);

    // Per ADR-0005, signatures and bodies live in separate Salsa queries.
    // We need the HEADER (mapping name + shape IRI + source binding) from
    // `lower_to_hir` and the BODY (property list) from `body(db, mapping)`.
    //
    // The `MappingLoc.index` is per-kind dense (audited in `def_map.rs`'s
    // contract block, matching `body()`'s filter-then-nth convention), so
    // it's also the dense index into `lower_to_hir(file).mappings`.
    let dm = def_map(db, file);
    let mapping_locs = dm.mappings(db);
    let Some(dense_idx) = mapping_locs.iter().position(|loc| *loc == mapping) else {
        // Foreign MappingLoc — emit an empty graph rather than panicking.
        return MirGraph::new(db, Vec::new());
    };
    let hir = lower_to_hir(db, file);
    let mappings = hir.mappings(db);
    let Some(m) = mappings.get(dense_idx) else {
        return MirGraph::new(db, Vec::new());
    };
    let body = body(db, mapping);
    let prefixes = dm.prefixes(db);

    // Source row type: prefer the type-checker's CSVW-derived `source_row`
    // (CORE-05). On a type error or a schema-less source, fall back to the
    // Phase 1 `Record({id, name})` so codegen still emits output and the
    // walking-skeleton stays byte-identical (NEVER panic — Pitfall: type
    // errors must not regress `fossil compile`).
    let row_type = typecheck_mapping(db, mapping).map_or_else(
        |_| phase1_row_type(db),
        |out| out.source_row(db).unwrap_or_else(|| phase1_row_type(db)),
    );

    let mut ops: Vec<Op<'db>> = Vec::with_capacity(4);

    // 0: Source — resolve the real URI + format from the mapping's source
    // binding (STDL-06). `dm` (def_map, file-keyed) is already read above; the
    // `lookup_source_call` is a pure read off that same handle, so this adds NO
    // new per-mapping fan-out (RESEARCH Pitfall 3 / STATE.md "Do NOT"). The
    // constructor NAME (`io.csv`/`io.json`/`io.parquet`) selects the format; the
    // constructor's first positional string is the URI. A malformed/unknown
    // binding falls back to the Phase-1 csv hardcode so we never panic and the
    // walking-skeleton stays byte-identical when the binding IS `io.csv`.
    let (uri, format) = resolve_source(dm, db, &m.source_binding);
    ops.push(Op::Source {
        uri,
        format,
        row_type,
    });

    // 1: Extend — attach the IRI template result as a column named "iri".
    //
    // SC#4 / P-CRIT-4 (CORE-10): the IRI template's `${.field}` placeholders are
    // the un-statically-dischargeable check this phase mitigates — a NULL field
    // would yield a malformed IRI. We resolve the source LINE for the `iri`
    // property HERE (during lowering) from the per-mapping `spans` side table so
    // codegen stays a pure render (RESEARCH Pitfall 3 — never read
    // `parse(db, file)` in the per-mapping path; `spans` and `mapping_cst_node`
    // are already barrier-routed, so this does NOT widen the per-mapping
    // fan-out). `iri_span_line` is then threaded into the field-ref wrapping in
    // `lower_iri_template` → `lower_placeholder`, where each `${.field}` ColRef
    // becomes `Expr::Assert { name: "iri_template_unbound", span_line, inner }`.
    let iri_span_line = iri_property_line(db, mapping, body);
    let iri_expr = lower_iri_property(m, body, prefixes, db, iri_span_line).unwrap_or_else(|| {
        // No `iri = ...` property; emit an empty literal to keep the upstream
        // Extend present for the TripleEmit subjects to reference.
        Expr::LitString(SmolStr::default())
    });
    ops.push(Op::Extend {
        input: 0,
        field: SmolStr::new_static("iri"),
        expr: iri_expr,
    });
    let extend_idx = 1usize;

    // 2..N: one TripleEmit per non-`iri` predicate property (subject = the
    // shared `iri` column reference; object = the property RHS lowered to an
    // `Expr`). The `iri` column is produced by the Extend at `extend_idx`.
    let subject = Expr::ColRef {
        source: SmolStr::default(),
        column: SmolStr::new_static("iri"),
    };
    for prop in body.properties(db) {
        let PropertyKey::PrefixedName { iri } = &prop.key else {
            continue; // skip the `iri = ...` property (handled by the Extend)
        };
        // Object positions are NOT wrapped in an assertion this phase (SC#4
        // candidate 1 is the IRI-template subject only; object-side cardinality
        // assertions are deferred — see the `<context>` note in 04-06-PLAN).
        let object = lower_property_value(&prop.value, &m.source_binding, prefixes, None);
        ops.push(Op::TripleEmit {
            input: extend_idx,
            subject: subject.clone(),
            predicate: iri.clone(),
            object,
            graph: None,
        });
    }

    // Final: Sink — GraphAr terminal, consuming the last op (the last
    // TripleEmit if any properties exist, else the Extend).
    let sink_input = ops.len() - 1;
    ops.push(Op::Sink {
        input: sink_input,
        sink: SinkRef::GraphAr,
    });

    // Run the structural rewriting engine (R1–R6) as PLAIN RUST inside this
    // tracked frame — NOT a separate tracked query (ADR-0010), so the
    // per-mapping fan-out is unchanged. hello.fossil's `Source → Extend →
    // TripleEmit → Sink` matches none of the R1–R6 triggers, so its SQL stays
    // byte-identical.
    let graph = MirGraph::new(db, ops);
    crate::rewrite::rewrite(db, graph)
}

/// Resolve the `Op::Source` URI + [`SourceFormat`] for a mapping's source
/// binding (STDL-06).
///
/// Reads the `(constructor, uri)` pair off the already-loaded [`DefMap`]
/// (file-keyed — NO new per-mapping fan-out, RESEARCH Pitfall 3). The
/// constructor name selects the format:
/// - `io.csv` → [`SourceFormat::Csv`] (DuckDB `read_csv_auto`)
/// - `io.json` → [`SourceFormat::Json`] (DuckDB `read_json_auto`)
/// - `io.parquet` → [`SourceFormat::Parquet`] (DuckDB `read_parquet`)
///
/// A binding with no recognisable `io.*("...")` call, or an unknown
/// constructor, degrades to the Phase-1 `examples/users.csv` / `Csv` so
/// `lower_to_mir` never panics on a malformed source. When the binding IS
/// `io.csv("examples/users.csv")` (the walking-skeleton `hello.fossil`) the
/// resolved value equals the old hardcode → byte-identical SQL.
#[allow(clippy::doc_markdown)] // read_csv_auto/read_json_auto/read_parquet are SQL fn names, not Rust items
fn resolve_source<'db>(
    dm: DefMap<'db>,
    db: &'db dyn fossil_base::Db,
    binding: &SmolStr,
) -> (SmolStr, SourceFormat) {
    let (constructor, uri) = dm.lookup_source_call(db, binding).unwrap_or((None, None));
    let format = match constructor.as_deref() {
        Some("io.json") => SourceFormat::Json,
        Some("io.parquet") => SourceFormat::Parquet,
        // `io.csv`, an unknown constructor, or no constructor → Csv (the
        // Phase-1 default; keeps malformed sources lowering rather than
        // panicking).
        _ => SourceFormat::Csv,
    };
    let uri = uri.unwrap_or_else(|| SmolStr::new_static("examples/users.csv"));
    (uri, format)
}

/// Phase 1 fallback row type: `Record({id: String, name: String})`.
///
/// Used when [`typecheck_mapping`] returns `Err` or the source declared no
/// CSVW `schema` (the walking-skeleton `hello.fossil` case has no `schema`
/// arg, so `TypeckOutput.source_row` is `None`).
fn phase1_row_type(db: &dyn fossil_base::Db) -> Ty<'_> {
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let record = Record::new(
        db,
        vec![
            RecordField {
                name: SmolStr::new_static("id"),
                ty: string_ty,
            },
            RecordField {
                name: SmolStr::new_static("name"),
                ty: string_ty,
            },
        ],
    );
    Ty::new(db, TyKind::Record(record))
}

/// Resolve the 1-based, MAPPING-RELATIVE source line of the `iri = ...`
/// property's RHS expression for the SC#4 named assertion (`line=<N>`).
///
/// # Why mapping-relative (RESEARCH Pitfall 3)
///
/// The `spans` side table records MAPPING-RELATIVE byte offsets (rowan's
/// `new_root` resets offsets to zero — see `fossil_hir::spans` offset-semantics
/// doc). We deliberately count newlines in the MAPPING's own CST text (read via
/// [`mapping_cst_node`], the SAME barrier `body`/`spans` already read) rather
/// than the whole-file source. Reading `parse(db, file)` for a file-absolute
/// line would tie this per-mapping query to the whole-file CST and break
/// `MAX_PER_MAPPING_FAN_OUT = 1`. The file-absolute conversion (if ever wanted)
/// is a display concern for the CLI/codegen wrapper, where a whole-file read
/// already exists. For Phase 4 the mapping-relative line is snapshot-stable and
/// sufficient.
///
/// `ExprId(i)` is the i-th lowered property's RHS (the `body`/`spans` indexing
/// convention). We find the `iri` property's position in `body.properties` and
/// look up its span. On a missing span (defensive — should not happen for a
/// well-formed mapping) we fall back to line `0`.
fn iri_property_line<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    body: HirBody<'db>,
) -> u32 {
    let Some(iri_idx) = body
        .properties(db)
        .iter()
        .position(|p| matches!(p.key, PropertyKey::Iri))
    else {
        return 0;
    };
    let expr_id = ExprId(u32::try_from(iri_idx).unwrap_or(u32::MAX));
    let Some(span) = spans(db, mapping).get(db, expr_id) else {
        return 0;
    };
    // Count newlines in the mapping body text up to the span start → 1-based
    // mapping-relative line. `mapping_cst_node` is the barrier `spans` itself
    // reads, so its offsets and the span offsets share the same origin.
    let Some(node) = mapping_cst_node(db, mapping).syntax() else {
        return 0;
    };
    let text = node.text().to_string();
    let start = (span.start as usize).min(text.len());
    // A mapping body has ≤~50 lines; a plain byte scan is fine here — the
    // `bytecount` crate clippy suggests would be a needless dependency for this
    // cold (per-mapping, once) path.
    #[allow(clippy::naive_bytecount)]
    let newlines = text.as_bytes()[..start]
        .iter()
        .filter(|&&b| b == b'\n')
        .count();
    u32::try_from(newlines + 1).unwrap_or(u32::MAX)
}

/// Find the `iri = ...` property in a mapping's body and lower its template
/// value to a concat-chain of [`Expr`]. `span_line` is the resolved
/// mapping-relative source line (see [`iri_property_line`]) used to populate the
/// SC#4 `Expr::Assert` wrappers on the template's `${.field}` placeholders.
fn lower_iri_property<'db>(
    m: &HirMapping,
    body: HirBody<'db>,
    prefixes: &[PrefixEntry],
    db: &'db dyn fossil_base::Db,
    span_line: u32,
) -> Option<Expr<'db>> {
    let prop = body
        .properties(db)
        .iter()
        .find(|p| matches!(p.key, PropertyKey::Iri))?;
    Some(lower_property_value(
        &prop.value,
        &m.source_binding,
        prefixes,
        Some(span_line),
    ))
}

/// Lower a property RHS [`HirExpr`] (one of the 4 leaf forms) to a typed
/// [`Expr`]. `FieldRef` → `ColRef`; `StringLit` → `LitString`;
/// `Template` → the concat-chain of literals + column refs;
/// `PrefixedName` → `LitString` of the resolved IRI.
///
/// `assert_line` is `Some(N)` when lowering an IRI-template subject context
/// (the `iri = ...` property), `None` for object positions. When `Some`, each
/// `${.field}` placeholder `ColRef` in a `Template` is wrapped in
/// `Expr::Assert { name: "iri_template_unbound", span_line: N, .. }` (SC#4 —
/// the un-statically-dischargeable NULL-field check).
///
/// # CODEGEN-LOWERING-01 (Phase 8 carry-forward closed in Phase 9-01)
///
/// Field-ref [`Expr::ColRef`] values emit `source: SmolStr::default()` (empty)
/// — NOT the source-binding name. [`fossil_codegen::render_expr`]
/// (sql.rs:763-773) substitutes its `default_source` argument (the view name
/// derived from `derive_view_name(uri)` — e.g. `hello` for
/// `@examples/hello.csv`) for any empty source. Letting the codegen's
/// view-name substitution be the single source of truth keeps binding names
/// out of emitted SQL — they are a HIR concern, not a SQL concern. Before the
/// fix, lowering emitted `source: source_binding` (the binding name `users`),
/// which `render_expr` honoured verbatim, producing `users.id` even when the
/// URI was `@examples/hello.csv` (view aliased as `hello`) — DuckDB-WASM
/// rejected with `Binder Error: Referenced table "users" not found! Candidate
/// tables: "hello"`. See
/// `.planning/phases/08-playground-react-library-v0-1/deferred-items.md`
/// (CODEGEN-LOWERING-01) and
/// `.planning/phases/09-playground-polish-differentiators/09-01-PLAN.md`.
fn lower_property_value<'db>(
    value: &HirExpr,
    source_binding: &SmolStr,
    prefixes: &[PrefixEntry],
    assert_line: Option<u32>,
) -> Expr<'db> {
    match value {
        HirExpr::FieldRef(field) => Expr::ColRef {
            // CODEGEN-LOWERING-01: empty source — codegen's `default_source`
            // (the view name from `derive_view_name(uri)`) substitutes.
            source: SmolStr::default(),
            column: field.clone(),
        },
        HirExpr::StringLit(s) => Expr::LitString(s.clone()),
        HirExpr::Template(raw) => lower_iri_template(raw, source_binding, prefixes, assert_line),
        // A `PrefixedName` RHS resolved to its full IRI by the HIR; render it
        // as a literal string value (the IRI text).
        HirExpr::PrefixedName { iri } => Expr::LitString(iri.clone()),
    }
}

/// IRI-template lowering. Parses the raw template token text (including
/// surrounding backticks and `${...}` placeholders) and emits a left-leaning
/// [`Expr::Concat`] chain of literal segments and column references.
///
/// Recognised placeholder forms:
/// - `${prefix:}` → the prefix's resolved IRI from the per-file prefix table
/// - `${.field}` → [`Expr::ColRef`] against the mapping's source binding
fn lower_iri_template<'db>(
    raw: &str,
    source_binding: &SmolStr,
    prefixes: &[PrefixEntry],
    assert_line: Option<u32>,
) -> Expr<'db> {
    // Strip the surrounding backticks (the HIR keeps them on the raw token).
    let inner = raw.trim_start_matches('`').trim_end_matches('`');

    let mut parts: Vec<Expr<'db>> = Vec::new();
    let mut cursor = 0usize;
    while cursor < inner.len() {
        // Find the next `${` placeholder start.
        let Some(open_off) = inner[cursor..].find("${") else {
            // No more placeholders — push the remaining literal tail.
            let tail = &inner[cursor..];
            if !tail.is_empty() {
                parts.push(Expr::LitString(SmolStr::from(tail)));
            }
            break;
        };
        let open = cursor + open_off;
        // Push the literal segment before the placeholder.
        if open > cursor {
            let lit = &inner[cursor..open];
            parts.push(Expr::LitString(SmolStr::from(lit)));
        }
        // Find the matching `}`.
        let after_open = open + 2; // skip "${"
        let Some(close_off) = inner[after_open..].find('}') else {
            // Unterminated placeholder; treat the rest as a literal tail.
            let tail = &inner[open..];
            parts.push(Expr::LitString(SmolStr::from(tail)));
            break;
        };
        let close = after_open + close_off;
        let placeholder = &inner[after_open..close];
        parts.push(lower_placeholder(
            placeholder,
            source_binding,
            prefixes,
            assert_line,
        ));
        cursor = close + 1; // skip past `}`
    }

    fold_concat_left(parts)
}

/// Lower one placeholder body (the text between `${` and `}`).
///
/// - `.field` → `ColRef` against the mapping's source binding, wrapped in an
///   `Expr::Assert { name: "iri_template_unbound", .. }` when `assert_line` is
///   `Some` (the IRI-template subject context — SC#4 / P-CRIT-4). The assertion
///   name is a FIXED `snake_case` identifier; NO type text is ever interpolated
///   (RESEARCH Pitfall 5).
/// - `prefix:` → the prefix's resolved IRI from the per-file prefix table.
/// - anything else → echo the placeholder back as a literal.
fn lower_placeholder<'db>(
    body: &str,
    source_binding: &SmolStr,
    prefixes: &[PrefixEntry],
    assert_line: Option<u32>,
) -> Expr<'db> {
    // `source_binding` is retained as a parameter for symmetry with
    // `lower_property_value` and future multi-source disambiguation. It is NOT
    // emitted into the ColRef — see CODEGEN-LOWERING-01 doc on
    // `lower_property_value` above. The binding name stays a HIR concern;
    // codegen's `default_source` (view name) is the SQL qualifier.
    let _ = source_binding;
    if let Some(field) = body.strip_prefix('.') {
        let col_ref = Expr::ColRef {
            // CODEGEN-LOWERING-01: empty source — codegen substitutes the
            // view name via `default_source`.
            source: SmolStr::default(),
            column: SmolStr::from(field),
        };
        // SC#4: in the IRI-template subject context, a `${.field}` whose value
        // may be NULL at runtime would produce a malformed IRI. We cannot
        // statically discharge non-nullness here (Optional-tracking on
        // `source_row` is thin in v0.1), so CONSERVATIVELY wrap every template
        // field ref in a named runtime assertion. Codegen renders this as
        // `CASE WHEN <field> IS NOT NULL THEN <field> ELSE error(...) END`.
        return match assert_line {
            Some(line) => Expr::Assert {
                name: SmolStr::new_static("iri_template_unbound"),
                span_line: line,
                inner: Box::new(col_ref),
            },
            None => col_ref,
        };
    }
    // `prefix:` form — resolve against the real prefix table (replaces the
    // Phase 1 hardcoded `ex:` branch). The prefix table stores `name` WITHOUT
    // the trailing colon.
    if let Some(name) = body.strip_suffix(':')
        && let Some(entry) = prefixes.iter().find(|e| e.name.as_str() == name)
    {
        return Expr::LitString(entry.iri.clone());
    }
    Expr::LitString(SmolStr::from(format!("${{{body}}}")))
}

/// Fold a list of expression parts into a left-leaning Concat chain with
/// adjacent-literal fusion: `[Lit("a"), Lit("b"), Col]` → `Concat(Lit("ab"), Col)`.
///
/// Fusion is required for codegen to produce the snapshot SQL
/// (`'https://example.org/user/'`, not `'https://example.org/' || 'user/'`).
fn fold_concat_left<'db>(parts: Vec<Expr<'db>>) -> Expr<'db> {
    let mut fused: Vec<Expr<'db>> = Vec::with_capacity(parts.len());
    for part in parts {
        match (fused.last_mut(), &part) {
            (Some(Expr::LitString(prev)), Expr::LitString(next)) => {
                let merged = SmolStr::from(format!("{prev}{next}"));
                *prev = merged;
            }
            _ => fused.push(part),
        }
    }
    if fused.is_empty() {
        return Expr::LitString(SmolStr::default());
    }
    let mut iter = fused.into_iter();
    let mut acc = iter.next().expect("non-empty after the empty check above");
    for next in iter {
        acc = Expr::Concat(Box::new(acc), Box::new(next));
    }
    acc
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_hir::def_map::def_map;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    #[test]
    fn lower_to_mir_for_hello_produces_4_ops() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("hello has one mapping");
        let mir = lower_to_mir(&db, mapping);
        let ops = mir.ops(&db);
        assert_eq!(ops.len(), 4, "expected 4 ops, got {}", ops.len());

        // Op 0: Source
        match &ops[0] {
            Op::Source {
                uri,
                format,
                row_type: _,
            } => {
                assert_eq!(uri.as_str(), "examples/users.csv");
                assert_eq!(*format, SourceFormat::Csv);
            }
            other => panic!("expected Source at index 0, got {other:?}"),
        }

        // Op 1: Extend(iri = 'https://example.org/user/' || users.id)
        match &ops[1] {
            Op::Extend { input, field, expr } => {
                assert_eq!(*input, 0);
                assert_eq!(field.as_str(), "iri");
                match expr {
                    // SC#4: the `${.id}` field ref is now wrapped in a named
                    // runtime assertion (iri_template_unbound) — the `inner` is
                    // the original ColRef. The literal prefix is unchanged.
                    Expr::Concat(l, r) => match (l.as_ref(), r.as_ref()) {
                        (
                            Expr::LitString(lit),
                            Expr::Assert {
                                name,
                                span_line,
                                inner,
                            },
                        ) => {
                            assert_eq!(lit.as_str(), "https://example.org/user/");
                            assert_eq!(name.as_str(), "iri_template_unbound");
                            assert!(
                                *span_line >= 1,
                                "span_line must be a resolved 1-based line, got {span_line}"
                            );
                            match inner.as_ref() {
                                Expr::ColRef { source, column } => {
                                    // CODEGEN-LOWERING-01: ColRef.source is
                                    // empty so codegen substitutes the view
                                    // name (the URI stem from
                                    // `derive_view_name`). Binding names
                                    // (`users`) are a HIR concern, not a SQL
                                    // concern.
                                    assert_eq!(source.as_str(), "");
                                    assert_eq!(column.as_str(), "id");
                                }
                                other => panic!("expected ColRef inside Assert, got {other:?}"),
                            }
                        }
                        (lo, ro) => panic!(
                            "expected Concat(LitString, Assert(ColRef)), got Concat({lo:?}, {ro:?})"
                        ),
                    },
                    other => panic!("expected Concat for iri expr, got {other:?}"),
                }
            }
            other => panic!("expected Extend at index 1, got {other:?}"),
        }

        // Op 2: TripleEmit(subject=ColRef(iri), predicate=ex:name, object=ColRef(name))
        match &ops[2] {
            Op::TripleEmit {
                input,
                subject,
                predicate,
                object,
                graph,
            } => {
                assert_eq!(*input, 1);
                assert!(
                    matches!(subject, Expr::ColRef { column, .. } if column.as_str() == "iri"),
                    "expected subject ColRef(iri), got {subject:?}"
                );
                assert_eq!(predicate.as_str(), "https://example.org/name");
                assert!(
                    matches!(object, Expr::ColRef { source, column }
                        if source.as_str() == "" && column.as_str() == "name"),
                    "expected object ColRef(<empty>.name), got {object:?}"
                );
                assert_eq!(*graph, None);
            }
            other => panic!("expected TripleEmit at index 2, got {other:?}"),
        }

        // Op 3: Sink(GraphAr)
        match &ops[3] {
            Op::Sink { input, sink } => {
                assert_eq!(*input, 2);
                assert_eq!(*sink, SinkRef::GraphAr);
            }
            other => panic!("expected Sink at index 3, got {other:?}"),
        }
    }

    /// STDL-06: a mapping reading from an `io.json("...")` / `io.parquet("...")`
    /// binding lowers `Op::Source` with the real URI from the binding and the
    /// format selected by the constructor name.
    fn lower_source_for(src: &str) -> (SmolStr, SourceFormat) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir(&db, mapping);
        match &mir.ops(&db)[0] {
            Op::Source { uri, format, .. } => (uri.clone(), *format),
            other => panic!("expected Source at index 0, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // `${ex:}` is template syntax, not a Rust format arg
    fn lower_to_mir_resolves_json_source() {
        let src = "\
prefix ex: <https://example.org/>

rows := io.json(\"a.json\")

User : ex:Person from rows
    iri = `${ex:}user/${.id}`
    ex:name = .name
";
        let (uri, format) = lower_source_for(src);
        assert_eq!(uri.as_str(), "a.json");
        assert_eq!(format, SourceFormat::Json);
    }

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // `${ex:}` is template syntax, not a Rust format arg
    fn lower_to_mir_resolves_parquet_source() {
        let src = "\
prefix ex: <https://example.org/>

rows := io.parquet(\"a.parquet\")

User : ex:Person from rows
    iri = `${ex:}user/${.id}`
    ex:name = .name
";
        let (uri, format) = lower_source_for(src);
        assert_eq!(uri.as_str(), "a.parquet");
        assert_eq!(format, SourceFormat::Parquet);
    }

    #[test]
    fn lower_to_mir_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("hello has one mapping");
        let a = lower_to_mir(&db, mapping);
        let b = lower_to_mir(&db, mapping);
        assert_eq!(a, b);
    }

    #[test]
    fn template_lowering_handles_trailing_literal() {
        // `${.id}/profile` → Concat(ColRef(users.id), LitString("/profile"))
        let raw = "`${.id}/profile`";
        let binding = SmolStr::new_static("users");
        // `assert_line = None` → object-position lowering (no Assert wrapper).
        let lowered: Expr<'_> = lower_iri_template(raw, &binding, &[], None);
        match lowered {
            Expr::Concat(l, r) => {
                assert!(matches!(l.as_ref(), Expr::ColRef { .. }));
                assert!(matches!(r.as_ref(), Expr::LitString(s) if s.as_str() == "/profile"));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn template_field_ref_wraps_in_named_assertion_when_subject_context() {
        // `assert_line = Some(N)` (the IRI-template subject context) → the
        // `${.id}` field ref is wrapped in `Assert { name:
        // "iri_template_unbound", span_line: N }` (SC#4 / P-CRIT-4). The
        // assertion NAME is a fixed snake_case identifier — never type text.
        let raw = "`${.id}/profile`";
        let binding = SmolStr::new_static("users");
        let lowered: Expr<'_> = lower_iri_template(raw, &binding, &[], Some(3));
        match lowered {
            Expr::Concat(l, r) => {
                match l.as_ref() {
                    Expr::Assert {
                        name,
                        span_line,
                        inner,
                    } => {
                        assert_eq!(name.as_str(), "iri_template_unbound");
                        assert_eq!(*span_line, 3);
                        assert!(matches!(inner.as_ref(), Expr::ColRef { column, .. }
                            if column.as_str() == "id"));
                    }
                    other => panic!("expected Assert(ColRef), got {other:?}"),
                }
                assert!(matches!(r.as_ref(), Expr::LitString(s) if s.as_str() == "/profile"));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // `${ex:}` is template syntax, not a Rust format arg
    fn template_lowering_resolves_real_prefix_table() {
        // `${ex:}user/${.id}` with ex -> https://example.org/ resolves the
        // prefix from the table (not a hardcoded branch).
        let raw = "`${ex:}user/${.id}`";
        let binding = SmolStr::new_static("users");
        let prefixes = vec![PrefixEntry {
            name: SmolStr::new_static("ex"),
            iri: SmolStr::new_static("https://example.org/"),
        }];
        // `assert_line = None` → bare ColRef (object-position semantics) so this
        // test stays focused on prefix-table resolution + literal fusion.
        let lowered: Expr<'_> = lower_iri_template(raw, &binding, &prefixes, None);
        match lowered {
            Expr::Concat(l, r) => {
                assert!(
                    matches!(l.as_ref(), Expr::LitString(s) if s.as_str() == "https://example.org/user/"),
                    "expected fused prefix+literal, got {:?}",
                    l.as_ref()
                );
                assert!(
                    matches!(r.as_ref(), Expr::ColRef { column, .. } if column.as_str() == "id")
                );
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }
}
