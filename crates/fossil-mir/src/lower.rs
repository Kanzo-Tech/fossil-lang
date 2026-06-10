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

use fossil_descriptors_output::OutputDescriptorKind;
use fossil_hir::body::{ExprId, HirBody, body, mapping_cst_node};
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::{DefMap, PrefixEntry, def_map};
use fossil_hir::lower::lower_to_hir;
use fossil_hir::spans::spans;
use fossil_hir::ty::RecordField;
use fossil_hir::{HirExpr, HirMapping, MappingLoc, Primitive, PropertyKey, Record, Ty, TyKind};
use smol_str::SmolStr;

use crate::graph::MirGraph;
use crate::op::{Expr, Op, SinkRef, SourceFormat, VProp};

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
        binding: m.source_binding.clone(),
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

/// Property-graph-canonical lowering (paso 2): `Source → EmitVertex → Sink`.
///
/// Branch-by-abstraction alongside [`lower_to_mir`] (which still emits the
/// `Source → Extend → TripleEmit* → Sink` triple path — UNCHANGED, so the legacy
/// SQL codegen + corpus stay byte-identical). This increment covers the
/// VERTEX-only shape: the `iri = ...` template becomes the vertex `id`, and every
/// other property becomes a [`VProp`]. EDGE classification (a property whose
/// value points at another shape → [`Op::EmitEdge`]) + the descriptor-driven
/// cardinality/types refinement are the NEXT increment (they need the
/// `OutputDescriptorKind` / skeleton-match the codegen `vertex_edge_decomp`
/// already has). Reuses the same `resolve_source` / `lower_iri_property` /
/// `lower_property_value` helpers so there is ZERO duplicated lowering logic.
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db mirrors lower_to_mir
pub fn lower_to_mir_pg<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> MirGraph<'db> {
    let file = mapping.file(db);
    let dm = def_map(db, file);
    let Some(dense_idx) = dm.mappings(db).iter().position(|loc| *loc == mapping) else {
        return MirGraph::new(db, Vec::new());
    };
    let hir = lower_to_hir(db, file);
    let Some(m) = hir.mappings(db).get(dense_idx) else {
        return MirGraph::new(db, Vec::new());
    };
    let body = body(db, mapping);
    let prefixes = dm.prefixes(db);
    let row_type = typecheck_mapping(db, mapping).map_or_else(
        |_| phase1_row_type(db),
        |out| out.source_row(db).unwrap_or_else(|| phase1_row_type(db)),
    );
    // v0.1: every prop is typed String (the legacy path types nothing either —
    // codegen ignores `Ty`). The descriptor-driven type refinement is the next
    // increment; the backend derives the GraphAr/xsd spelling from `Ty`.
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));

    let mut ops: Vec<Op<'db>> = Vec::with_capacity(3);

    // 0: Source.
    let (uri, format) = resolve_source(dm, db, &m.source_binding);
    ops.push(Op::Source {
        uri,
        format,
        row_type,
        binding: m.source_binding.clone(),
    });

    // `id` = the IRI template (the same Expr the legacy Extend would carry).
    let iri_span_line = iri_property_line(db, mapping, body);
    let id = lower_iri_property(m, body, prefixes, db, iri_span_line)
        .unwrap_or_else(|| Expr::LitString(SmolStr::default()));

    // Subject-template skeleton of EVERY mapping in the file → its vertex type.
    // A property whose backtick-template skeleton matches one of these is a
    // foreign key → an edge to that type (reuses the shared skeleton-matching).
    let subject_skeletons: Vec<(String, SmolStr)> = dm
        .mappings(db)
        .iter()
        .enumerate()
        .filter_map(|(i, loc)| {
            let ty = SmolStr::new(local_name(hir.mappings(db).get(i)?.shape_iri.as_str()));
            Some((crate::skeleton::subject_template_skeleton(db, *loc)?, ty))
        })
        .collect();

    // Classify each non-`iri` property: FieldRef/StringLit → vertex prop;
    // IRI-template that resolves to another subject → edge; dangling template /
    // constant prefixed-name → neither (v0.1 — mirrors synthesize_sink_plan).
    // (a) Type refinement: a FieldRef prop carries the source field's type
    // (CSVW-refined when the source declares a `schema`; String otherwise — same
    // as the legacy path, which types nothing). The backend derives the
    // GraphAr/xsd spelling from `ty`. Cardinality stays `single_valued = true`
    // here (the ShEx-descriptor refinement that would set multi-valued needs the
    // descriptor wired into the lowering — a later increment).
    let field_ty = |field: &str| -> Ty<'db> {
        if let TyKind::Record(rec) = row_type.kind(db) {
            if let Some(f) = rec.fields(db).iter().find(|f| f.name == field) {
                return f.ty;
            }
        }
        string_ty
    };

    let mut props: Vec<VProp<'db>> = Vec::new();
    let mut edges: Vec<(SmolStr, SmolStr, SmolStr, Expr<'db>)> = Vec::new();
    for prop in body.properties(db) {
        let PropertyKey::PrefixedName { iri } = &prop.key else {
            continue; // the `iri = ...` property is the vertex id
        };
        let pred_local = SmolStr::new(local_name(iri));
        match &prop.value {
            HirExpr::FieldRef(field) => props.push(VProp {
                name: pred_local,
                value: lower_property_value(&prop.value, &m.source_binding, prefixes, None),
                ty: field_ty(field.as_str()),
                rdf_uri: Some(iri.clone()),
                single_valued: true,
            }),
            HirExpr::StringLit(_) => props.push(VProp {
                name: pred_local,
                value: lower_property_value(&prop.value, &m.source_binding, prefixes, None),
                ty: string_ty,
                rdf_uri: Some(iri.clone()),
                single_valued: true,
            }),
            HirExpr::Template(t) => {
                let skel = crate::skeleton::template_skeleton(t.as_str());
                if let Some((_, dst_type)) = subject_skeletons.iter().find(|(s, _)| *s == skel) {
                    let dst_id = lower_property_value(&prop.value, &m.source_binding, prefixes, None);
                    edges.push((pred_local, dst_type.clone(), iri.clone(), dst_id));
                }
                // non-matching template → dangling, no edge (v0.1)
            }
            HirExpr::PrefixedName { .. } => {} // constant IRI → not an edge
        }
    }

    let type_name = SmolStr::new(local_name(&m.shape_iri));

    // 1: EmitVertex. All emit ops read the source relation at index 0 (the Sink
    // is nominal — the backend walks every EmitVertex/EmitEdge op, as the legacy
    // codegen walks every TripleEmit).
    ops.push(Op::EmitVertex {
        input: 0,
        type_name: type_name.clone(),
        rdf_type: Some(m.shape_iri.clone()),
        id: id.clone(),
        dedup: true,
        props,
    });

    // 2..N: one EmitEdge per resolved foreign-key template.
    for (pred_local, dst_type, pred_iri, dst_id) in edges {
        ops.push(Op::EmitEdge {
            input: 0,
            edge_type: pred_local,
            rdf_uri: Some(pred_iri),
            src_type: type_name.clone(),
            dst_type,
            src_id: id.clone(),
            dst_id,
            single_valued: true,
        });
    }

    // Final: Sink consuming the last emit op.
    let sink_input = ops.len() - 1;
    ops.push(Op::Sink {
        input: sink_input,
        sink: SinkRef::GraphAr,
    });

    let graph = MirGraph::new(db, ops);
    crate::rewrite::rewrite(db, graph)
}

/// Refine an agnostic [`lower_to_mir_pg`] op list with the program-resident
/// **output descriptor** (ShEx): reclassify the `EmitVertex`'s properties into
/// edges + set their cardinality from the shape's constraints.
///
/// The agnostic lowering types every property as a single-valued vertex column
/// (it has no shape knowledge — `iri`-templates aside, an `ex:hasProject = .x`
/// `FieldRef` looks like a column). The ShEx descriptor is what knows that
/// `ex:hasProject` is a **shape-ref** (→ a typed edge) and that `*`/`+`
/// cardinality is **multi-valued**. This is the same edge-vs-property decision
/// `fossil-sinks`'s `vertex_edge_decomp` makes for the SQL writer, sharing the
/// one authority ([`ResolvedConstraint::edge_target`] +
/// [`Cardinality::is_single_valued`]).
///
/// Passed the descriptor as an **argument** (ADR-0018 — the descriptor is NEVER
/// read through `Db::system()`), so this is a plain `Vec<Op>`→`Vec<Op>` pass: it
/// never constructs a [`MirGraph`] (a Salsa tracked struct, illegal outside a
/// tracked query) and never touches Salsa. `AcceptAll` (the walking-skeleton /
/// no-shape case) returns the ops unchanged — the template-skeleton edges the
/// agnostic lowering already produced stand.
#[must_use]
pub fn apply_output_shape<'db>(
    ops: &[Op<'db>],
    descriptor: &OutputDescriptorKind,
) -> Vec<Op<'db>> {
    let OutputDescriptorKind::ShEx(desc) = descriptor else {
        return ops.to_vec();
    };

    // The vertex's shape IRI keys its constraint table; without it (or a shape
    // the descriptor doesn't declare) there is nothing to refine.
    let shape_iri = ops.iter().find_map(|o| match o {
        Op::EmitVertex { rdf_type, .. } => rdf_type.as_ref().map(SmolStr::as_str),
        _ => None,
    });
    let Some(binding) = shape_iri.and_then(|iri| desc.lookup_shape_str(iri)) else {
        return ops.to_vec();
    };
    let constraint_for = |predicate: &str| {
        binding
            .constraints
            .iter()
            .find(|c| c.predicate.to_string() == predicate)
    };

    // Rebuild: Source(s) + the refined EmitVertex + existing edges + the new
    // shape-ref edges, then the Sink (re-pointed at the new last op).
    let mut head: Vec<Op<'db>> = Vec::with_capacity(ops.len());
    let mut new_edges: Vec<Op<'db>> = Vec::new();
    let mut sink: Option<Op<'db>> = None;

    for op in ops {
        match op {
            Op::Sink { sink: kind, .. } => sink = Some(Op::Sink { input: 0, sink: *kind }),
            Op::EmitVertex {
                input,
                type_name,
                rdf_type,
                id,
                dedup,
                props,
            } => {
                let mut kept: Vec<VProp<'db>> = Vec::with_capacity(props.len());
                for p in props {
                    let predicate = p.rdf_uri.as_deref().unwrap_or_default();
                    match constraint_for(predicate) {
                        Some(c) if c.edge_target().is_some() => {
                            let dst = c.edge_target().expect("edge_target checked");
                            new_edges.push(Op::EmitEdge {
                                input: *input,
                                edge_type: p.name.clone(),
                                rdf_uri: p.rdf_uri.clone(),
                                src_type: type_name.clone(),
                                dst_type: SmolStr::new(local_name(&dst)),
                                src_id: id.clone(),
                                dst_id: p.value.clone(),
                                single_valued: c.cardinality.is_single_valued(),
                            });
                        }
                        Some(c) => kept.push(VProp {
                            single_valued: c.cardinality.is_single_valued(),
                            ..p.clone()
                        }),
                        None => kept.push(p.clone()),
                    }
                }
                head.push(Op::EmitVertex {
                    input: *input,
                    type_name: type_name.clone(),
                    rdf_type: rdf_type.clone(),
                    id: id.clone(),
                    dedup: *dedup,
                    props: kept,
                });
            }
            other => head.push(other.clone()),
        }
    }

    head.extend(new_edges);
    if let Some(Op::Sink { sink: kind, .. }) = sink {
        let input = head.len().saturating_sub(1);
        head.push(Op::Sink { input, sink: kind });
    }
    head
}

/// Local name of an IRI: the segment after the last `#` or `/` (falls back to
/// the whole string for a bare term). Used for the vertex `type_name` + prop
/// names in [`lower_to_mir_pg`].
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// Resolve the `Op::Source` URI + [`SourceFormat`] for a mapping's source
/// binding (STDL-06).
///
/// Reads the `(constructor, uri)` pair off the already-loaded [`DefMap`]
/// (file-keyed — NO new per-mapping fan-out, RESEARCH Pitfall 3). The format is
/// resolved by looking the constructor up in [`fossil_registry::SOURCE_KINDS`]
/// (W1 single source of truth) — no string-matching here. A
/// [`SourceLowering::NativeReader`] maps exhaustively to a [`SourceFormat`] (a
/// new reader variant is a compile error until handled); a
/// [`SourceLowering::Provider`] becomes `SourceFormat::Provider { name }`.
///
/// A binding with no recognisable `io.*("...")` call degrades to the Phase-1
/// `examples/users.csv` / `Csv` so `lower_to_mir` never panics on a malformed
/// source. An `io.<name>` not in `SOURCE_KINDS` still resolves to
/// `Provider { name }` so the runtime reports "unknown provider" rather than
/// silently mis-reading it. When the binding IS `io.csv("examples/users.csv")`
/// (the walking-skeleton `hello.fossil`) the resolved value equals the old
/// hardcode → byte-identical SQL.
// The nested `match` over the constructor + its lowering reads clearer than the
// `map_or_else` the nursery lint suggests (the Some arm is itself a match).
#[allow(clippy::option_if_let_else)]
fn resolve_source<'db>(
    dm: DefMap<'db>,
    db: &'db dyn fossil_base::Db,
    binding: &SmolStr,
) -> (SmolStr, SourceFormat) {
    let (constructor, uri) = dm.lookup_source_call(db, binding).unwrap_or((None, None));
    let format = match constructor.as_deref() {
        Some(c) => match fossil_registry::source_kind(c) {
            Some(kind) => match kind.lowering {
                fossil_registry::SourceLowering::NativeReader(r) => native_reader_format(r),
                fossil_registry::SourceLowering::Provider => SourceFormat::Provider {
                    name: SmolStr::new(kind.short_name),
                },
            },
            // Unrecognised `io.<name>` → a provider the runtime will reject by
            // name (clearer than silently reading it as CSV); anything else →
            // the Phase-1 Csv default so malformed sources still lower.
            None => c.strip_prefix("io.").map_or(SourceFormat::Csv, |name| {
                SourceFormat::Provider {
                    name: SmolStr::new(name),
                }
            }),
        },
        None => SourceFormat::Csv,
    };
    let uri = uri.unwrap_or_else(|| SmolStr::new_static("examples/users.csv"));
    (uri, format)
}

/// Exhaustive [`NativeReader`](fossil_registry::NativeReader) → [`SourceFormat`]
/// map. A new native reader is a compile error here until handled (the W1
/// invariant: source dispatch can't silently forget a format).
const fn native_reader_format(r: fossil_registry::NativeReader) -> SourceFormat {
    match r {
        fossil_registry::NativeReader::CsvAuto => SourceFormat::Csv,
        fossil_registry::NativeReader::JsonAuto => SourceFormat::Json,
        fossil_registry::NativeReader::Parquet => SourceFormat::Parquet,
    }
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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
                ..
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir(&db, mapping);
        match &mir.ops(&db)[0] {
            Op::Source { uri, format, .. } => (uri.clone(), format.clone()),
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
