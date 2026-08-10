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
use fossil_graph_schema::Cardinality as GsCardinality;
use fossil_graph_schema::Primitive;
use fossil_hir::body::{ExprId, HirBody, body, mapping_cst_node};
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::{DefMap, PrefixEntry, def_map};
use fossil_hir::lower::lower_to_hir;
use fossil_hir::spans::spans;
use fossil_hir::{HirExpr, HirMapping, MappingLoc, PropertyKey, Record, Ty, TyKind};
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
    let span = mapping_span(db, mapping);
    // `def_map` builds `MappingLoc::new(db, file, i)` in order, so a mapping's
    // interned index IS its position in that table. The linear search this
    // replaces was the second quadratic in this function — one full scan per
    // mapping, for an answer the key already carried.
    let dense_idx = mapping.index(db);
    if dense_idx >= dm.mappings(db).len() {
        return poisoned(
            db,
            fossil_base::bug(db, span, "mapping is absent from its own DefMap"),
        );
    }
    let hir = lower_to_hir(db, file);
    let Some(m) = hir.mappings(db).get(dense_idx) else {
        return poisoned(
            db,
            fossil_base::bug(db, span, "mapping has no HIR at its DefMap index"),
        );
    };
    let body = body(db, mapping);
    let prefixes = dm.prefixes(db);
    // A typecheck failure taints (it already emitted the diagnostic). A source
    // that simply declares no schema is NOT a failure — `source_row` is `None`
    // for every program without a CSVW/ShEx schema — so it yields an empty row
    // type, and `field_ty` types those columns `String` as before. What it must
    // never do is invent field names: the old `Record({id, name})` default made
    // unrelated sources look like they had `id` and `name` columns.
    let row_type = match typecheck_mapping(db, mapping) {
        Err(eg) => return poisoned(db, eg),
        Ok(out) => out.source_row(db).unwrap_or_else(|| untyped_row(db)),
    };
    // v0.1: every prop is typed String (the legacy path types nothing either —
    // codegen ignores `Ty`). The descriptor-driven type refinement is the next
    // increment; the backend derives the GraphAr/xsd spelling from `Ty`.
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));

    let mut ops: Vec<Op<'db>> = Vec::with_capacity(3);

    // 0..k: the source relation. One `Op::Source` for a binding that reads a
    // file; `Source` plus one op per verb when the binding is a pipeline.
    let source =
        match lower_source_chain(db, dm, file, &m.source_binding, span, prefixes, &mut ops, 0) {
            Ok(c) => c,
            Err(eg) => return poisoned(db, eg),
        };
    let source_idx = source.last;

    // `id` = the vertex IRI. No `iri` property (or one that does not lower) is
    // fatal: the old empty-string default produced vertices whose subject was
    // `""`, which dedups every row of the mapping into a single blank node.
    let iri_span_line = iri_property_line(db, mapping, body);
    let Some(id) = lower_iri_property(m, body, prefixes, db, iri_span_line) else {
        return poisoned(
            db,
            fossil_base::delay_span_bug(
                db,
                span,
                "mapping has no usable `iri = ...` property, so its vertices have no subject",
            ),
        );
    };

    // Subject-template skeleton of EVERY mapping in the file → its vertex type.
    // A property whose backtick-template skeleton matches one of these is a
    // foreign key → an edge to that type. File-keyed: building it here, once
    // per mapping, is what made this function quadratic — see the query's own
    // doc for the measurement.
    let subject_skeletons = crate::skeleton::subject_skeletons(db, file);

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
        if let TyKind::Record(rec) = row_type.kind(db)
            && let Some(f) = rec.fields(db).iter().find(|f| f.name == field)
        {
            return f.ty;
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
            // The qualified spelling lowers identically: the checker has
            // already established that the binding IS this mapping's source
            // (ADR-0057, ninth amendment), so only the column reaches MIR.
            HirExpr::ColumnRef { column: field, .. } | HirExpr::FieldRef(field) => {
                props.push(VProp {
                    name: pred_local,
                    value: lower_property_value(db, &prop.value, &m.source_binding, prefixes, None),
                    ty: field_ty(field.as_str()),
                    rdf_uri: Some(iri.clone()),
                    single_valued: true,
                });
            }
            // A string literal and a conditional both produce a String column
            // here: the conditional's branches agree by the time the checker is
            // done, and without a source row that agreed type is String.
            HirExpr::StringLit(_) | HirExpr::Ternary { .. } => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, prefixes, None),
                ty: string_ty,
                rdf_uri: Some(iri.clone()),
                single_valued: true,
            }),
            HirExpr::Template(t) => {
                let skel = crate::skeleton::template_skeleton(t.as_str());
                if let Some((_, dst_type)) = subject_skeletons.iter().find(|(s, _)| *s == skel) {
                    let dst_id =
                        lower_property_value(db, &prop.value, &m.source_binding, prefixes, None);
                    edges.push((pred_local, dst_type.clone(), iri.clone(), dst_id));
                }
                // non-matching template → dangling, no edge (v0.1)
            }
            HirExpr::PrefixedName { .. } => {} // constant IRI → not an edge
            // A computed property: the value is whatever the function returns,
            // typed by its catalog entry. It is never an edge — an edge is a
            // reference to another shape's subject, and v0.1 has no function
            // that produces one.
            HirExpr::Call { func, .. } => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, prefixes, None),
                ty: call_result_ty(db, func),
                rdf_uri: Some(iri.clone()),
                single_valued: true,
            }),
            // A comparison is a Bool column; an integer literal an Integer one.
            // The type is the operator's and the literal's, not the row's.
            HirExpr::BinOp { .. } => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, prefixes, None),
                ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
                rdf_uri: Some(iri.clone()),
                single_valued: true,
            }),
            HirExpr::IntLit(_) => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, prefixes, None),
                ty: Ty::new(db, TyKind::Primitive(Primitive::Integer)),
                rdf_uri: Some(iri.clone()),
                single_valued: true,
            }),
        }
    }

    let type_name = SmolStr::new(local_name(&m.shape_iri));

    // 1: EmitVertex. All emit ops read the source relation at index 0 (the Sink
    // is nominal — the backend walks every EmitVertex/EmitEdge op, as the legacy
    // codegen walks every TripleEmit).
    ops.push(Op::EmitVertex {
        input: source_idx,
        type_name: type_name.clone(),
        rdf_type: Some(m.shape_iri.clone()),
        id: id.clone(),
        dedup: true,
        props,
    });

    // 2..N: one EmitEdge per resolved foreign-key template.
    for (pred_local, dst_type, pred_iri, dst_id) in edges {
        ops.push(Op::EmitEdge {
            input: source_idx,
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

    MirGraph::new(db, ops, None)
}

/// A tainted, op-less graph. `eg` is the [`fossil_base::ErrorGuaranteed`] whose
/// construction already accumulated the explaining `Diagnostic` (P-CRIT-4), so
/// the caller never has to remember to emit one.
fn poisoned(db: &dyn fossil_base::Db, eg: fossil_base::ErrorGuaranteed) -> MirGraph<'_> {
    MirGraph::new(db, Vec::new(), Some(eg))
}

/// The row type of a source that declares no schema: a record with no known
/// fields. Distinct from a *failure* to type the source — callers of
/// `field_ty` fall back to `String`, exactly as they did before.
fn untyped_row(db: &dyn fossil_base::Db) -> Ty<'_> {
    Ty::new(db, TyKind::Record(Record::new(db, Vec::new())))
}

/// Span of the mapping's own CST node, for diagnostics anchored at the mapping
/// rather than at one of its properties. Reads the `mapping_cst_node` barrier
/// that `body`/`spans` already read, so it adds no per-mapping Salsa fan-out.
fn mapping_span<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> fossil_base::Span {
    mapping_cst_node(db, mapping).syntax().map_or_else(
        || fossil_base::Span::new(0, 0),
        |node| {
            let r = node.text_range();
            fossil_base::Span::new(r.start().into(), r.end().into())
        },
    )
}

/// Refine an agnostic [`lower_to_mir_pg`] op list with the program-resident
/// **output descriptor** (ShEx): reclassify the `EmitVertex`'s properties into
/// edges + set their cardinality from the shape's constraints.
///
/// The agnostic lowering types every property as a single-valued vertex column
/// (it has no shape knowledge — `iri`-templates aside, an `ex:hasProject = .x`
/// `FieldRef` value looks like a column). The `ShEx` descriptor is what knows that
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
pub fn apply_output_shape<'db>(ops: &[Op<'db>], descriptor: &OutputDescriptorKind) -> Vec<Op<'db>> {
    // Route every source schema language (ShEx / SHACL / accept-all) through
    // the one canonical output model. The executor classifies against this; it
    // never sees ShEx- or SHACL-specific types.
    let schema = descriptor.to_graph_schema();

    // The vertex's shape IRI keys its node type; without it (or a node the
    // model doesn't declare) there is nothing to refine.
    let shape_iri = ops.iter().find_map(|o| match o {
        Op::EmitVertex { rdf_type, .. } => rdf_type.as_ref().map(SmolStr::as_str),
        _ => None,
    });
    let Some(node) = shape_iri.and_then(|iri| schema.node_by_iri(iri)) else {
        return ops.to_vec();
    };

    // Rebuild: Source(s) + the refined EmitVertex + existing edges + the new
    // shape-ref edges, then the Sink (re-pointed at the new last op).
    let mut head: Vec<Op<'db>> = Vec::with_capacity(ops.len());
    let mut new_edges: Vec<Op<'db>> = Vec::new();
    let mut sink: Option<Op<'db>> = None;

    for op in ops {
        match op {
            Op::Sink { sink: kind, .. } => {
                sink = Some(Op::Sink {
                    input: 0,
                    sink: *kind,
                });
            }
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
                    // A predicate whose range is a shape (union) becomes one edge
                    // per destination type (`@<A> OR @<B>` → two edges). A literal
                    // / IRI-valued predicate stays a vertex property. Otherwise the
                    // skeleton property is kept untouched (accept-all).
                    let mut edges = schema.edges_from(&node.label, predicate).peekable();
                    if edges.peek().is_some() {
                        for e in edges {
                            new_edges.push(Op::EmitEdge {
                                input: *input,
                                edge_type: p.name.clone(),
                                rdf_uri: p.rdf_uri.clone(),
                                src_type: type_name.clone(),
                                dst_type: SmolStr::new(&e.destination),
                                src_id: id.clone(),
                                dst_id: p.value.clone(),
                                single_valued: matches!(e.cardinality, GsCardinality::Single),
                            });
                        }
                    } else if let Some(prop_def) = node.property_by_iri(predicate) {
                        kept.push(VProp {
                            single_valued: matches!(prop_def.cardinality, GsCardinality::Single),
                            ..p.clone()
                        });
                    } else {
                        kept.push(p.clone());
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
pub(crate) fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// The relation a mapping's `from` names, lowered into the op list.
///
/// # The naming rule, which the backend has to agree with
///
/// **Every named binding is a relation known by its name.** `Op::Source` is
/// known by its binding; a pipeline's result is known by the binding it defines.
/// In between, a `Filter` or a `Project` does not rename — the rows are the same
/// rows — so a predicate over `users |> where(.edad >= 18)` qualifies its columns
/// with `users`, while one written after a `join` qualifies them with the
/// pipeline's own name, because the join built a relation neither side was.
struct Chain {
    /// Index into the op list of the chain's last op — what an emit op reads.
    last: usize,
    /// The name the rendered SQL knows this relation by.
    relation: SmolStr,
}

/// A pipeline deriving from a pipeline deriving from … The cycle is already a
/// diagnostic in the checker (`fossil_hir::infer`), so reaching this depth here
/// means the graph got past type-checking, which is a bug and says so.
const MAX_CHAIN_DEPTH: usize = 32;

/// Lower the source binding a mapping reads into `ops`, following the pipeline
/// if it is one (ADR-0054). A binding that reads a file is one `Op::Source`; a
/// pipeline is its base's chain followed by one op per verb.
fn lower_source_chain<'db>(
    db: &'db dyn fossil_base::Db,
    dm: DefMap<'db>,
    file: fossil_base::SourceFile,
    binding: &SmolStr,
    span: fossil_base::Span,
    prefixes: &[fossil_hir::def_map::PrefixEntry],
    ops: &mut Vec<Op<'db>>,
    depth: usize,
) -> Result<Chain, fossil_base::ErrorGuaranteed> {
    use fossil_hir::lower::HirSourceOp;

    let hir = lower_to_hir(db, file);
    let pipe = hir
        .source_pipes(db)
        .iter()
        .find(|p| p.name == *binding)
        .cloned();

    let Some(pipe) = pipe else {
        let (uri, format) = resolve_source(dm, db, binding, span)?;
        // The row type of THIS binding, not of the mapping: a pipeline's mapping
        // sees the derived row, and the source underneath it still declares its
        // own. Any diagnostic this would raise was already raised by the
        // type-check, which poisons the graph before the lowering runs.
        // `.ok().flatten()`: the taint was already raised by the type-check,
        // which poisons the graph before the lowering runs, so here a failed row
        // is the same as an absent one — an untyped source, as before.
        let row_type = fossil_hir::infer::resolve_binding_row(db, file, binding.as_str(), 0)
            .ok()
            .flatten()
            .unwrap_or_else(|| untyped_row(db));
        ops.push(Op::Source {
            uri,
            format,
            row_type,
            binding: binding.clone(),
        });
        return Ok(Chain {
            last: ops.len() - 1,
            relation: binding.clone(),
        });
    };

    if depth >= MAX_CHAIN_DEPTH {
        return Err(fossil_base::bug(
            db,
            span,
            format!("the source pipeline `{binding}` recurses past the checker's own cycle guard"),
        ));
    }

    let mut chain = lower_source_chain(db, dm, file, &pipe.base, span, prefixes, ops, depth + 1)?;
    for op in &pipe.ops {
        match op {
            HirSourceOp::Where(pred) => {
                let pred = lower_property_value(db, pred, &chain.relation, prefixes, None);
                ops.push(Op::Filter {
                    input: chain.last,
                    pred,
                });
            }
            HirSourceOp::Select(cols) => ops.push(Op::Project {
                input: chain.last,
                cols: cols.clone(),
            }),
            HirSourceOp::Join { right, key } => {
                let right_chain =
                    lower_source_chain(db, dm, file, right, span, prefixes, ops, depth + 1)?;
                // `on = .k` is `USING (k)`: one equality between the same column
                // name on both sides, qualified by each side's relation. The
                // checker has already proved `k` is on both and types the same
                // (ADR-0054 §5), so this cannot be built wrong here.
                ops.push(Op::Join {
                    left: chain.last,
                    right: right_chain.last,
                    on: Expr::BinOp {
                        op: fossil_hir::CmpOp::Eq,
                        lhs: Box::new(Expr::ColRef {
                            source: chain.relation.clone(),
                            column: key.clone(),
                        }),
                        rhs: Box::new(Expr::ColRef {
                            source: right_chain.relation.clone(),
                            column: key.clone(),
                        }),
                        ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
                    },
                    kind: crate::op::JoinKind::Inner,
                    left_name: chain.relation.clone(),
                    right_name: right_chain.relation.clone(),
                });
                // A join builds a relation neither side was; from here on the
                // pipeline's own name is what qualifies its columns.
                chain.relation = pipe.name.clone();
            }
        }
        chain.last = ops.len() - 1;
    }
    // And the finished pipeline is the relation its binding names, which is what
    // the mapping's `from` and every property's column reference use.
    chain.relation = pipe.name.clone();
    Ok(chain)
}

/// Resolve the `Op::Source` URI + [`SourceFormat`] for a mapping's source
/// binding (STDL-06).
///
/// Reads the `(constructor, uri)` pair off the already-loaded [`DefMap`]
/// (file-keyed — NO new per-mapping fan-out, RESEARCH Pitfall 3). The format is
/// resolved by looking the constructor up in [`fossil_hir::stdlib::SOURCE_KINDS`]
/// (W1 single source of truth) — no string-matching here. A
/// [`SourceLowering::NativeReader`] maps exhaustively to a [`SourceFormat`] (a
/// new reader variant is a compile error until handled); a
/// [`SourceLowering::Provider`] becomes `SourceFormat::Provider { name }`.
///
/// A binding that resolves to no URI is an ERROR, not a default. It is reached
/// whenever the mapping reads `from` something that is not an `io.*("...")`
/// call — most often a derived binding such as
/// `x := Source |> seq.filter(...)`, which the parser accepts as a source
/// definition but which carries no constructor and no URI. Substituting a
/// default here is what silently pointed every such mapping at
/// `examples/users.csv` instead of the file the program named.
///
/// An `io.<name>` not in `SOURCE_KINDS` still resolves to `Provider { name }`
/// so the runtime reports "unknown provider" rather than mis-reading it as CSV.
// The nested `match` over the constructor + its lowering reads clearer than the
// `map_or_else` the nursery lint suggests (the Some arm is itself a match).
#[allow(clippy::option_if_let_else)]
fn resolve_source<'db>(
    dm: DefMap<'db>,
    db: &'db dyn fossil_base::Db,
    binding: &SmolStr,
    span: fossil_base::Span,
) -> Result<(SmolStr, SourceFormat), fossil_base::ErrorGuaranteed> {
    let (constructor, uri) = dm.lookup_source_call(db, binding).unwrap_or((None, None));
    let Some(uri) = uri else {
        return Err(fossil_base::delay_span_bug(
            db,
            span,
            match constructor.as_deref() {
                Some(c) => format!(
                    "`{binding}` is not a source: it is bound to `{c}`, which is not an \
                     `io.*(\"...\")` call, so there is no file to read. A mapping's `from` \
                     must name a binding declared as `{binding} := io.csv(\"...\")` (or \
                     io.json / io.parquet / io.rdf)."
                ),
                None => format!("`{binding}` is not a declared source binding"),
            },
        ));
    };
    let format = match constructor.as_deref() {
        Some(c) => match fossil_hir::stdlib::source_kind(c) {
            Some(kind) => match kind.lowering {
                fossil_hir::stdlib::SourceLowering::NativeReader(r) => native_reader_format(r),
                fossil_hir::stdlib::SourceLowering::Provider => SourceFormat::Provider {
                    name: SmolStr::new(kind.short_name),
                },
            },
            None => match c.strip_prefix("io.") {
                Some(name) => SourceFormat::Provider {
                    name: SmolStr::new(name),
                },
                None => {
                    return Err(fossil_base::delay_span_bug(
                        db,
                        span,
                        format!("`{c}` is not a source constructor; expected `io.*`"),
                    ));
                }
            },
        },
        None => {
            return Err(fossil_base::delay_span_bug(
                db,
                span,
                format!("source `{binding}` has a URI but no constructor to read it with"),
            ));
        }
    };
    Ok((uri, format))
}

/// Exhaustive [`NativeReader`](fossil_hir::stdlib::NativeReader) → [`SourceFormat`]
/// map. A new native reader is a compile error here until handled (the W1
/// invariant: source dispatch can't silently forget a format).
const fn native_reader_format(r: fossil_hir::stdlib::NativeReader) -> SourceFormat {
    match r {
        fossil_hir::stdlib::NativeReader::CsvAuto => SourceFormat::Csv,
        fossil_hir::stdlib::NativeReader::JsonAuto => SourceFormat::Json,
        fossil_hir::stdlib::NativeReader::Parquet => SourceFormat::Parquet,
    }
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
        db,
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
    db: &'db dyn fossil_base::Db,
    value: &HirExpr,
    source_binding: &SmolStr,
    prefixes: &[PrefixEntry],
    assert_line: Option<u32>,
) -> Expr<'db> {
    match value {
        HirExpr::ColumnRef { column: field, .. } | HirExpr::FieldRef(field) => Expr::ColRef {
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
        // The result type comes from the same catalog entry the checker typed
        // this call against — the backend derives the column's datatype from
        // it, so a call is no less typed than a column reference.
        HirExpr::IntLit(v) => Expr::LitInt(*v),
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => Expr::Ternary {
            cond: Box::new(lower_property_value(
                db,
                cond,
                source_binding,
                prefixes,
                assert_line,
            )),
            then: Box::new(lower_property_value(
                db,
                then,
                source_binding,
                prefixes,
                assert_line,
            )),
            otherwise: Box::new(lower_property_value(
                db,
                otherwise,
                source_binding,
                prefixes,
                assert_line,
            )),
            // The branch type is the conditional's; the checker proved they
            // agree. Without a source row neither branch types, and String is
            // what every other untyped property gets.
            ty: Ty::new(db, TyKind::Primitive(Primitive::String)),
        },
        HirExpr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op: *op,
            lhs: Box::new(lower_property_value(
                db,
                lhs,
                source_binding,
                prefixes,
                assert_line,
            )),
            rhs: Box::new(lower_property_value(
                db,
                rhs,
                source_binding,
                prefixes,
                assert_line,
            )),
            ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
        },
        HirExpr::Call { func, args } => Expr::Call {
            func: func.clone(),
            args: args
                .iter()
                .map(|a| lower_property_value(db, a, source_binding, prefixes, assert_line))
                .collect(),
            ty: call_result_ty(db, func),
        },
    }
}

/// The declared return type of a catalogued function, as a `Ty`.
///
/// A name that is not in the catalog cannot reach here — the checker rejects it
/// and poisons the mapping — so the fallback is unreachable in a program that
/// type-checked. It types `String` rather than panicking because lowering runs
/// on poisoned mappings too (the walking-skeleton invariant: never panic).
fn call_result_ty<'db>(db: &'db dyn fossil_base::Db, func: &SmolStr) -> Ty<'db> {
    fossil_hir::stdlib::stdlib()
        .lookup(func.as_str())
        .map_or_else(
            || Ty::new(db, TyKind::Primitive(Primitive::String)),
            |e| e.sig.ret.to_ty(db),
        )
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

    /// STDL-06: a mapping reading from an `io.json("...")` / `io.parquet("...")`
    /// binding lowers `Op::Source` with the real URI from the binding and the
    /// format selected by the constructor name.
    fn lower_source_for(src: &str) -> (SmolStr, SourceFormat) {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        match &mir.ops(&db)[0] {
            Op::Source { uri, format, .. } => (uri.clone(), format.clone()),
            other => panic!("expected Source at index 0, got {other:?}"),
        }
    }

    /// A mapping reading from a pipeline lowers the pipeline, and the emit ops
    /// read its LAST op — not index 0.
    ///
    /// `input: 0` was correct while a source was exactly one op, and it is the
    /// thing that silently drops a filter the day it stops being one: the graph
    /// still has the `Filter`, the vertices are still written, and every row the
    /// predicate excluded is in the corpus. So the assertion is on the wiring,
    /// not on the op list.
    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // `${ex:}` is template syntax, not a Rust format arg
    fn a_mapping_over_a_pipeline_reads_the_last_op_of_the_chain() {
        let src = "\
prefix ex: <https://example.org/>

users := io.csv(\"u.csv\")
adultos := users |> where(.edad >= 18)

User : ex:Person from adultos
    iri = `${ex:}user/${.id}`
    ex:name = .name
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        let ops = mir.ops(&db);

        assert!(
            matches!(&ops[0], Op::Source { binding, uri, .. }
                if binding.as_str() == "users" && uri.as_str() == "u.csv"),
            "index 0 is the base source, got {:?}",
            ops[0]
        );
        let Op::Filter { input, pred } = &ops[1] else {
            panic!("index 1 is the `where`, got {:?}", ops[1]);
        };
        assert_eq!(*input, 0, "the filter reads the source");
        // `.edad` reaches MIR as an UNQUALIFIED `ColRef` — CODEGEN-LOWERING-01,
        // the convention every property in the tree already follows, and which
        // the backend resolves against the relation the op reads.
        assert!(
            matches!(pred, Expr::BinOp { op: fossil_hir::CmpOp::Ge, lhs, .. }
                if matches!(&**lhs, Expr::ColRef { column, .. } if column.as_str() == "edad")),
            "the predicate reads `.edad`, got {pred:?}"
        );
        assert!(
            matches!(&ops[2], Op::EmitVertex { input, .. } if *input == 1),
            "the vertex reads the FILTER, not the source, got {:?}",
            ops[2]
        );
    }

    /// A join lowers to `Op::Join` with both sides sourced, `Inner`, and an
    /// equality on the one key name — `on = .k` is `USING (k)` (ADR-0054 §3).
    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // `${ex:}` is template syntax, not a Rust format arg
    fn a_join_lowers_to_an_inner_equi_join_on_one_name() {
        let src = "\
prefix ex: <https://example.org/>

pedidos := io.csv(\"o.csv\")
personas := io.csv(\"p.csv\")
ventas := pedidos |> join(personas, on = .persona_id)

Venta : ex:Person from ventas
    iri = `${ex:}venta/${.id}`
    ex:name = .nombre
";
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        let ops = mir.ops(&db);

        assert!(matches!(&ops[0], Op::Source { binding, .. } if binding.as_str() == "pedidos"));
        assert!(matches!(&ops[1], Op::Source { binding, .. } if binding.as_str() == "personas"));
        let Op::Join {
            left,
            right,
            on,
            kind,
            left_name,
            right_name,
        } = &ops[2]
        else {
            panic!("index 2 is the join, got {:?}", ops[2]);
        };
        assert_eq!((*left, *right), (0, 1));
        assert_eq!(*kind, crate::op::JoinKind::Inner);
        assert_eq!(left_name.as_str(), "pedidos");
        assert_eq!(right_name.as_str(), "personas");
        let Expr::BinOp {
            op: fossil_hir::CmpOp::Eq,
            lhs,
            rhs,
            ..
        } = on
        else {
            panic!("the condition is one equality, got {on:?}");
        };
        assert!(
            matches!(&**lhs, Expr::ColRef { source, column }
                if source.as_str() == "pedidos" && column.as_str() == "persona_id"),
            "got {lhs:?}"
        );
        assert!(
            matches!(&**rhs, Expr::ColRef { source, column }
                if source.as_str() == "personas" && column.as_str() == "persona_id"),
            "the key is the SAME name on both sides, got {rhs:?}"
        );
        assert!(matches!(&ops[3], Op::EmitVertex { input, .. } if *input == 2));
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
