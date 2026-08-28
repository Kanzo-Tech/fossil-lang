//! HIR → MIR lowering for the source-reachable operator subset.
//!
//! Consumes the per-mapping HEADER from [`fossil_hir::HirMapping`], the
//! per-mapping BODY from [`fossil_hir::body::body`] — a query of its own
//! because the `ItemTree` carries SIGNATURES ONLY, so editing one mapping's
//! body invalidates neither the file's item tree nor its sibling mappings —
//! and the per-mapping TYPES from
//! [`fossil_hir::check::typecheck_mapping`] (Phase 3, CORE-04..07). Emits a
//! [`MirGraph`] of the shape:
//! `Source → Extend(iri = ...) → TripleEmit* → Sink(GraphAr)`.
//!
//! # Reachability
//!
//! The IR defines the whole algebra; the surface reaches only part of it, and
//! that gap is deliberate — completeness of the IR is a statement about the
//! algebra, not about what a `.fossil` file can spell. Reachable from source
//! today: `Source`, `EmitVertex`, `EmitEdge` and `Sink` from the mapping body,
//! plus `Filter` / `Project` / `Join` from the pipeline verbs `where` /
//! `select` / `join` on a source binding. The remainder (`Extend` / `Rename` /
//! `Union` / `GroupBy` / `Aggregate` / `Distinct` / `Empty`) has no spelling in
//! the language and is exercised by direct `MirGraph` construction instead.
//!
//! # Phase 4 generalisations over the Phase 1 hardcodes
//!
//! - **Source row type** comes from [`fossil_hir::check::TypeckOutput`]'s
//!   `source_row` when type-checking succeeds; otherwise it
//!   falls back to the Phase 1 `Record({id, name})` so codegen still produces
//!   output (walking-skeleton preserved — never panic).
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
//! # CRITICAL barrier rule
//!
//! `lower_to_mir` may read `body(db, mapping)`, `typecheck_mapping(db, mapping)`
//! — all barrier-routed through `mapping_cst_node`.
//! It MUST NOT add a `parse(db, file)` read in the per-mapping path (would
//! break `MAX_PER_MAPPING_FAN_OUT = 1`). `def_map(db, file)` is file-keyed and
//! structurally stable across body-only edits, so the `def_map` reads here do
//! not widen the per-mapping fan-out.
//!
//! `tests/fan_out.rs` is what goes red. It edits one property of one mapping in
//! ten and counts `lower_to_mir_pg` re-executions off Salsa's own events: one.
//! The rule was written in three places in this file and measured in none —
//! `MAX_PER_MAPPING_FAN_OUT` exists only in two `fossil-hir` test files, over
//! `fossil-hir`'s queries, so adding the forbidden read here left the whole of
//! that crate green. It now takes the count from one to ten.
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

use fossil_graph_schema::Cardinality as GsCardinality;
use fossil_graph_schema::{GraphSchema, Primitive, local_name};
use fossil_hir::body::{ExprId, HirBody, body, mapping_cst_node};
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::{DefMap, def_map};
use fossil_hir::lower::{InterpolationPart, lower_to_hir};
use fossil_hir::spans::spans;
use fossil_hir::{HirExpr, HirMapping, MappingLoc, PropertyKey, Record, Ty, TyKind};
use smol_str::SmolStr;

use crate::graph::MirGraph;
use crate::op::{Expr, Op, SinkRef, SourceFormat, VProp};

/// Lower one [`fossil_hir::MappingLoc`] to a [`MirGraph`]: the
/// property-graph-canonical `Source → EmitVertex → Sink`, with the pipeline
/// verbs of the source binding (`Filter` / `Project` / `Join`) between the
/// `Source` and the `EmitVertex` when the binding is a pipeline.
///
/// The mapping's `@subject` interpolation becomes the vertex `id`; every other
/// property becomes a [`VProp`]. EDGE classification (a property whose value
/// points at another node type → [`Op::EmitEdge`]) and the cardinality/types
/// refinement are a SEPARATE pass, [`apply_output_shape`], because they need a
/// [`GraphSchema`] this query has no way to read: MIR is a property graph in
/// the middle and never learns which schema language is on either side of it.
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
    // **This ICE reached users, and it was never about the mapping it named.**
    // `HirFile::mappings` used to be FILTERED — one entry per mapping whose
    // header lowered, indexed as if it were one per `MAPPING` node — so a file
    // with a header `lower_to_hir` declined ran the whole vector short. The
    // shortest input that produced it is four bytes, `a:b`, which is a mapping
    // header with no `from`; the fourteen-row `ShExC` drain of
    // `fossil-wasm/tests/shape_document.rs` produced three, one per line the
    // parser recovered into a `MAPPING`. On a file with a broken mapping AND a
    // healthy one it was worse than an ICE and silent: mapping 0 was lowered
    // against mapping 1's signature, and mapping 1 — the correct one — was the
    // one that ran off the end and reported the bug.
    //
    // The vector is one slot per `MAPPING` node now. `Err` is a header
    // `lower_to_hir` declined, and it already said so, once, file-level; the
    // taint travels and nothing is reported twice. `None` is the two CST walks
    // disagreeing about how many mappings the file has, which is the only thing
    // here that was ever a compiler bug.
    let m = match hir.mappings(db).get(dense_idx) {
        Some(Ok(m)) => m,
        Some(Err(eg)) => {
            // The body is still checked. `body` is keyed by the mapping and
            // reads the CST directly, so it needs nothing the header failed to
            // give; forcing it here puts its diagnostics in THIS query's
            // dependency subtree, which is where `program_diagnostics` drains
            // them from. Without this line a mapping whose header is one token
            // wrong loses every report about the lines under it, which is the
            // failure `lower_mapping_node` already refuses for a shape name it
            // cannot resolve.
            let _ = body(db, mapping);
            return poisoned(db, *eg);
        }
        None => {
            return poisoned(
                db,
                fossil_base::bug(
                    db,
                    span,
                    "the HIR has no slot at this mapping's DefMap index: `def_map` and \
                     `lower_to_hir` disagree about how many MAPPING nodes the file has",
                ),
            );
        }
    };
    let body = body(db, mapping);
    // A typecheck failure taints (it already emitted the diagnostic). A source
    // that simply declares no schema is NOT a failure — `source_row` is `None`
    // for every program whose source declares no schema — so it yields an empty row
    // type, and `field_ty` types those columns `String` as before. What it must
    // never do is invent field names: the old `Record({id, name})` default made
    // unrelated sources look like they had `id` and `name` columns.
    // `predicates` is how the IRI gets here. `fossil-mir` used to strip one out
    // of `PropertyKey::PrefixedName`, which the CURIE had put there; a bare key
    // severs that supply and the shape document is the only thing that knows.
    // The checker already resolves the shape, so the table arrives through a
    // seam this function already reads — as a pair of strings, with no shape
    // vocabulary and no descriptor, so nothing `0e6898d` cut comes back.
    let (row_type, predicates) = match typecheck_mapping(db, mapping) {
        Err(eg) => return poisoned(db, eg),
        Ok(out) => (
            out.source_row(db).unwrap_or_else(|| untyped_row(db)),
            out.predicates(db).clone(),
        ),
    };
    // v0.1: every prop is typed String (the legacy path types nothing either —
    // codegen ignores `Ty`). The descriptor-driven type refinement is the next
    // increment; the backend derives the GraphAr/xsd spelling from `Ty`.
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));

    let mut ops: Vec<Op<'db>> = Vec::with_capacity(3);

    // 0..k: the source relation. One `Op::Source` for a binding that reads a
    // file; `Source` plus one op per verb when the binding is a pipeline.
    let source = match lower_source_chain(db, dm, file, &m.source_binding, span, &mut ops, 0) {
        Ok(c) => c,
        Err(eg) => return poisoned(db, eg),
    };
    let source_idx = source.last;

    // `id` = the vertex IRI. No `iri` property (or one that does not lower) is
    // fatal: the old empty-string default produced vertices whose subject was
    // `""`, which dedups every row of the mapping into a single blank node.
    let iri_span_line = iri_property_line(db, mapping, body);
    let Some(id) = lower_iri_property(m, body, db, iri_span_line) else {
        return poisoned(
            db,
            fossil_base::delay_span_bug(
                db,
                span,
                "mapping has no usable `iri = ...` property, so its vertices have no subject",
            ),
        );
    };

    // `subject_skeletons` was read here, and it is gone. An edge used to be
    // GUESSED: the skeleton of a property's IRI template — every per-row hole
    // replaced by a `\u{1}` marker — was compared against the skeleton of every
    // mapping's subject in the file, and a match made it a foreign key. That
    // comparison existed only because the identity rule was repeated per
    // mapping. There is now exactly ONE identity per type — every mapping that
    // produces `T` declares the same `@subject`, and disagreeing is an error —
    // so the guess becomes a lookup, and the lookup already exists downstream:
    // `apply_output_shape` classifies a predicate as an edge when the SHAPE
    // says its range is a shape. Naming a shape document is MANDATORY, so that
    // path is always available — which is what made deleting this one safe.

    // Classify each non-`iri` property: FieldRef/StringLit → vertex prop;
    // IRI-template that resolves to another subject → edge; dangling template /
    // constant prefixed-name → neither (v0.1 — mirrors synthesize_sink_plan).
    // (a) Type refinement: a FieldRef prop carries the source field's type
    // (refined when the source declares a schema; String otherwise — same
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
    for prop in body.properties(db) {
        let PropertyKey::Name(name) = &prop.key else {
            continue; // `@subject` is the vertex id, not a predicate
        };
        // The short name IS the column name — it used to be computed here from
        // the IRI and is now what the author wrote. The IRI is the lookup.
        let pred_local = name.clone();
        let iri = &predicates
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, i)| i.clone());
        // `buyer = Person(User.email)` becomes the target type's identity
        // template with this row's values in its holes, and from here down it IS
        // that interpolation — the same shape a hand-written IRI template has,
        // which is why `apply_output_shape` needs no new case to turn it into an
        // `Op::EmitEdge`. This is the substitution `subject_skeletons` used to
        // GUESS at by comparing templates between mappings; one identity per
        // type — the same `@subject` in every mapping that produces it — turns
        // the guess into a lookup.
        let value = resolve_edges(db, file, &prop.value);
        let prop = &fossil_hir::HirProperty {
            key: prop.key.clone(),
            value,
        };
        // The column's declared TYPE, per variant — and no wildcard, so a new
        // variant cannot inherit someone else's type by falling through.
        #[deny(clippy::wildcard_enum_match_arm)]
        match &prop.value {
            // `null` is refused as a property value by the checker (its type is
            // comparable with everything and assignable to nothing), so this is
            // an invariant written down rather than a case.
            HirExpr::NullLit => {}
            // The qualified spelling lowers identically: the checker has
            // already established that the binding IS this mapping's source,
            // so only the column reaches MIR. (`orders.user_id` is the form
            // that survives; the anonymous `.field` row is being deleted.)
            HirExpr::ColumnRef { column: field, .. } | HirExpr::FieldRef(field) => {
                props.push(VProp {
                    name: pred_local,
                    value: lower_property_value(db, &prop.value, &m.source_binding, None),
                    ty: field_ty(field.as_str()),
                    rdf_uri: iri.clone(),
                    single_valued: true,
                });
            }
            // A string literal, a conditional and an interpolation all produce
            // a String column here: the conditional's branches agree by the
            // time the checker is done, and without a source row that agreed
            // type is String. An interpolation joined this arm when the edge
            // guess went — whether it is an EDGE is the shape's answer, taken
            // by `apply_output_shape`, not a fact about its text.
            HirExpr::StringLit(_) | HirExpr::Ternary { .. } | HirExpr::Interpolation(_) => props
                .push(VProp {
                    name: pred_local,
                    value: lower_property_value(db, &prop.value, &m.source_binding, None),
                    ty: string_ty,
                    rdf_uri: iri.clone(),
                    single_valued: true,
                }),
            // A computed property: the value is whatever the function returns,
            // typed by its catalog entry. It is never an edge — an edge is a
            // reference to another shape's subject, and v0.1 has no function
            // that produces one.
            HirExpr::Call { func, .. } => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, None),
                ty: call_result_ty(db, func),
                rdf_uri: iri.clone(),
                single_valued: true,
            }),
            // A comparison is a Bool column; a literal is its own type. An
            // arithmetic expression is NEITHER — `net = Row.gross -
            // Row.discount` over two `xsd:float` columns is a float column, and
            // this arm used to answer `Bool` for every `BinOp` there was. It was
            // right while `+` did not lower and became wrong the moment it did:
            // the property would have been written, with a value DuckDB computes
            // as a double and a declared type of boolean, which is the silent
            // half of a wrong answer.
            HirExpr::BinOp { .. } | HirExpr::UnaryOp { .. } => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, None),
                ty: operator_ty(db, &prop.value, &field_ty),
                rdf_uri: iri.clone(),
                single_valued: true,
            }),
            HirExpr::IntLit(_) => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, None),
                ty: Ty::new(db, TyKind::Primitive(Primitive::Integer)),
                rdf_uri: iri.clone(),
                single_valued: true,
            }),
            HirExpr::FloatLit(_) => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, None),
                ty: Ty::new(db, TyKind::Primitive(Primitive::Float)),
                rdf_uri: iri.clone(),
                single_valued: true,
            }),
            HirExpr::BoolLit(_) => props.push(VProp {
                name: pred_local,
                value: lower_property_value(db, &prop.value, &m.source_binding, None),
                ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
                rdf_uri: iri.clone(),
                single_valued: true,
            }),
            // `resolve_edges` above replaced every one of these. Reaching it
            // means the target had no template, which `typecheck_mapping`
            // refuses — and a refused mapping never gets here, because its
            // `Err` poisons this graph at the top of the function.
            HirExpr::Edge { target, .. } => {
                return poisoned(
                    db,
                    fossil_base::bug(
                        db,
                        span,
                        format!(
                            "an edge to `{target}` reached MIR unresolved: it has no identity \
                             template and the checker did not refuse the mapping"
                        ),
                    ),
                );
            }
        }
    }

    let type_name = SmolStr::new(local_name(&m.shape_iri));

    // 1: EmitVertex. All emit ops read the source relation at index 0 (the Sink
    // is nominal — the backend walks every EmitVertex/EmitEdge op, as the legacy
    // codegen walks every TripleEmit).
    ops.push(Op::EmitVertex {
        input: source_idx,
        type_name,
        rdf_type: Some(m.shape_iri.clone()),
        id: id.clone(),
        dedup: true,
        props,
    });

    // Final: Sink consuming the last emit op.
    let sink_input = ops.len() - 1;
    ops.push(Op::Sink {
        input: sink_input,
        sink: SinkRef::GraphAr,
    });

    MirGraph::new(db, ops, None)
}

/// A tainted, op-less graph. `eg` is the [`fossil_base::ErrorGuaranteed`] whose
/// construction already accumulated the explaining `Diagnostic`, so
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

/// Refine an agnostic [`lower_to_mir_pg`] op list against a
/// [`GraphSchema`]: reclassify the `EmitVertex`'s properties into edges + set
/// their cardinality from the schema's node/edge types.
///
/// The agnostic lowering types every property as a single-valued vertex column
/// (it has no schema knowledge — `iri`-templates aside, an `hasProject = .x`
/// `FieldRef` value looks like a column). The schema is what knows that
/// `ex:hasProject` **references another node type** (→ a typed edge) and that its
/// cardinality is **multi-valued**. This is the same edge-vs-property decision
/// `fossil-sinks`'s `vertex_edge_decomp` makes for the SQL writer, taken against
/// the one shared model.
///
/// Takes the **schema**, not the document that produced it: a caller holding a
/// `ShEx` or SHACL descriptor calls `to_graph_schema()` itself. MIR is a property
/// graph in the middle and never learns which output format or schema language
/// is on either side of it — the dependency says so, not just the intent.
///
/// The schema arrives as an **argument** and is NEVER read through
/// `Db::system()`. That is what keeps this a plain `Vec<Op>`→`Vec<Op>` pass:
/// it never constructs a [`MirGraph`] (a Salsa tracked struct, illegal outside
/// a tracked query), never touches Salsa, and so cannot widen any query's
/// fan-out. An empty schema (the walking-skeleton / accept-all case) returns
/// the ops unchanged — the edges the agnostic lowering already produced stand.
#[must_use]
pub fn apply_output_shape<'db>(ops: &[Op<'db>], schema: &GraphSchema) -> Vec<Op<'db>> {
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

/// The relation a mapping's `from` names, lowered into the op list.
///
/// # The naming rule, which the backend has to agree with
///
/// **A relation is addressed by the qualifiers its columns actually carry, and
/// there may be more than one.** `Op::Source` is qualified by its binding; a
/// `Filter`, a `Project` or a `Distinct` does not rename, so the rows keep the
/// qualifiers they had. A `join` keeps BOTH sides addressable — which is what
/// lets a body write `Purchase.amount` beside `User.email` — so it is the one
/// verb that makes this a set rather than a name. A `union` and a `group_by` are
/// the two that introduce the pipeline's own name, because each re-qualifies
/// what it produces (`Op::Union` wholly, `Op::GroupBy` its aggregates).
///
/// This field was a single `SmolStr` set to the pipeline's name after a join and
/// again at the end of every chain, and that name qualified nothing: see
/// [`crate::op::JoinSide`] for the two programs it made unplannable.
struct Chain {
    /// Index into the op list of the chain's last op — what an emit op reads.
    last: usize,
    /// Every name the rendered SQL knows this relation's columns by, in the
    /// order they became addressable.
    relations: Vec<SmolStr>,
}

impl Chain {
    /// The name an unqualified reference falls back to. Every `ColumnRef` the
    /// surface can write is qualified (`lower_property_value` copies the
    /// binding the author wrote), so this reaches nothing that renders a
    /// qualifier today; it is the base binding, which is what the single
    /// `relation` field held before the first join.
    fn primary(&self) -> SmolStr {
        self.relations.first().cloned().unwrap_or_default()
    }

    /// Make `name` addressable, without repeating one that already is.
    fn add(&mut self, name: &SmolStr) {
        if !self.relations.contains(name) {
            self.relations.push(name.clone());
        }
    }
}

/// A pipeline deriving from a pipeline deriving from … The cycle is already a
/// diagnostic in the checker (`fossil_hir::infer`), so reaching this depth here
/// means the graph got past type-checking, which is a bug and says so.
const MAX_CHAIN_DEPTH: usize = 32;

/// Lower the source binding a mapping reads into `ops`, following the pipeline
/// if it is one. A binding that reads a file is one `Op::Source`; a pipeline is
/// its base's chain followed by one op per verb.
fn lower_source_chain<'db>(
    db: &'db dyn fossil_base::Db,
    dm: DefMap<'db>,
    file: fossil_base::SourceFile,
    binding: &SmolStr,
    span: fossil_base::Span,
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
            relations: vec![binding.clone()],
        });
    };

    if depth >= MAX_CHAIN_DEPTH {
        return Err(fossil_base::bug(
            db,
            span,
            format!("the source pipeline `{binding}` recurses past the checker's own cycle guard"),
        ));
    }

    let mut chain = lower_source_chain(db, dm, file, &pipe.base, span, ops, depth + 1)?;
    for op in &pipe.ops {
        match op {
            HirSourceOp::Where(pred) => {
                let pred = lower_property_value(db, pred, &chain.primary(), None);
                ops.push(Op::Filter {
                    input: chain.last,
                    pred,
                });
            }
            // The binding travels with the column. `select` names a QUALIFIED
            // column (open question 4, decided 2026-08-14), and after a join
            // that is the only thing that says which side `id` came from.
            HirSourceOp::Select(cols) => ops.push(Op::Project {
                input: chain.last,
                cols: cols
                    .iter()
                    .map(|c| crate::op::ProjectedColumn {
                        source: c.binding.clone(),
                        column: c.column.clone(),
                    })
                    .collect(),
            }),
            HirSourceOp::Join { right, alias, on } => {
                let right_chain = lower_source_chain(db, dm, file, right, span, ops, depth + 1)?;
                // The condition is a PREDICATE the author writes, so there is
                // no key to synthesise: it lowers like any other expression.
                //
                // It is lowered in the LEFT relation's scope. Its column
                // references are qualified (`Purchase.user_id`,
                // `User.id`), so each one already carries the relation it
                // belongs to and the scope only supplies the default.
                let on = lower_property_value(db, on, &chain.primary(), None);
                // `Node.join(Node as Other, …)` — the self-join alias is the
                // second name for the same source, and it travels as an alias
                // rather than as the right side's name because the backend has
                // to know the difference: an alias is a RE-qualification of that
                // relation, and no alias means the relation keeps the
                // qualification its own columns are already addressed by.
                let right_side = crate::op::JoinSide {
                    input: right_chain.last,
                    relations: right_chain.relations.clone(),
                    alias: alias.clone(),
                };
                ops.push(Op::Join {
                    left: crate::op::JoinSide {
                        input: chain.last,
                        relations: chain.relations.clone(),
                        alias: None,
                    },
                    right: right_side.clone(),
                    on,
                    kind: crate::op::JoinKind::Inner,
                });
                // A join builds a relation neither side was, and it leaves BOTH
                // sides addressable — nothing is re-qualified and nothing is
                // dropped, so the qualifiers of the result are the two sides'
                // together. This assigned the pipeline's own name, which
                // qualified no column of either side and is what made a second
                // join over this one unplannable.
                for name in right_side.names() {
                    chain.add(name);
                }
            }
            HirSourceOp::Distinct => ops.push(Op::Distinct { input: chain.last }),
            // The keys keep the relation each was written against; the
            // aggregates get the pipeline's, because a group's total came from
            // no source and there is nothing to qualify it with. `agg_fn` is
            // the CATALOGUE's answer to which aggregate a call is, so this arm
            // never spells one.
            HirSourceOp::GroupBy { keys, aggs } => {
                let reg = fossil_hir::stdlib::stdlib();
                let aggs = aggs
                    .iter()
                    .filter_map(|a| {
                        let row = reg.lookup(a.func.as_str())?;
                        Some(crate::op::AggSpec {
                            out_field: a.out.clone(),
                            agg_fn: row.agg_fn()?,
                            column: crate::op::ProjectedColumn {
                                source: a.column.binding.clone(),
                                column: a.column.column.clone(),
                            },
                            ty: row.sig.ret.scalar()?.to_ty(db),
                        })
                    })
                    .collect();
                ops.push(Op::GroupBy {
                    input: chain.last,
                    keys: keys
                        .iter()
                        .map(|k| crate::op::ProjectedColumn {
                            source: k.binding.clone(),
                            column: k.column.clone(),
                        })
                        .collect(),
                    aggs,
                    relation: pipe.name.clone(),
                });
                // The keys keep the qualifier each was written with and the
                // aggregates are aliased under the pipeline's name, so both are
                // addressable after it — `Totals.total` beside `Order.customer`.
                chain
                    .relations
                    .retain(|r| keys.iter().any(|k| k.binding == *r));
                chain.add(&pipe.name);
            }
            // Like a join, a union builds a relation neither side was — and
            // unlike a join it does not keep the two sides addressable, because
            // a row of the result came from one of them and nothing says which.
            // The pipeline's name is what both sides are re-qualified under.
            HirSourceOp::Union { right } => {
                let right_chain = lower_source_chain(db, dm, file, right, span, ops, depth + 1)?;
                ops.push(Op::Union {
                    left: chain.last,
                    right: right_chain.last,
                    relation: pipe.name.clone(),
                });
                // A union DOES collapse to one name: `DataFusion` hands the
                // result back with no qualifier at all, so `Op::Union`
                // re-qualifies the whole thing and neither side survives.
                chain.relations = vec![pipe.name.clone()];
            }
        }
        chain.last = ops.len() - 1;
    }
    // And the finished pipeline is addressed by whatever its verbs left
    // addressable — NOT by its own binding.
    //
    // This assigned `pipe.name` unconditionally, and that was the second half of
    // the same mistake: `Adults := User.where(…)` produces rows still qualified
    // `User` (an `Op::Filter` renames nothing), so a chain ending here answered
    // to a name no column carried. It is why
    // `Purchase.join(Adults, on = Purchase.user_id == User.id)` refused `User`
    // as "neither input (`Purchase`, `Adults`)" — while `/docs/design/algebra`
    // and `JoinSide`'s own doc comment both describe that program as working.
    // Only `union` and `group_by` put the pipeline's name on a column, and each
    // now says so in its own arm.
    Ok(chain)
}

/// Resolve the `Op::Source` URI + [`SourceFormat`] for a mapping's source
/// binding (STDL-06).
///
/// Reads the `(constructor, uri)` pair off the already-loaded [`DefMap`]
/// (file-keyed — NO new per-mapping fan-out). The format is
/// resolved by looking the constructor up in the provider registry
/// ([`fossil_base::providers`]) — no string-matching here. A
/// [`RowReader::Native`](fossil_base::RowReader::Native) maps exhaustively to a
/// [`SourceFormat`] (a new reader variant is a compile error until handled); a
/// [`RowReader::Materialised`](fossil_base::RowReader::Materialised) becomes
/// `SourceFormat::Provider { name }`.
///
/// A binding that resolves to no URI is an ERROR, not a default. It is reached
/// whenever the mapping reads `from` something that is not an `io.*("...")`
/// call — most often a derived binding such as
/// `x := Source.where(...)`, which the parser accepts as a source
/// definition but which carries no constructor and no URI. Substituting a
/// default here is what silently pointed every such mapping at
/// `examples/users.csv` instead of the file the program named.
///
/// An `io.<name>` the host did not install still resolves to `Provider { name }`
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
        Some(c) => match fossil_base::provider(fossil_base::providers::installed(db), c) {
            Some(row) => match row.reads_rows {
                Some(fossil_base::RowReader::Native(r)) => native_reader_format(r),
                // Materialised outside the reader (`io.rdf`), or a row that does
                // not read rows at all (`from` a `io.shex` binding) — both are
                // `Provider { name }` here. The second is already a diagnostic
                // with a span from `fossil_hir::lower::check_provider`; poisoning
                // it a second time would report one mistake twice.
                _ => SourceFormat::Provider {
                    name: SmolStr::new(row.name),
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

/// Exhaustive [`NativeReader`](fossil_base::NativeReader) → [`SourceFormat`]
/// map. A new native reader is a compile error here until handled (source
/// dispatch can't silently forget a format).
const fn native_reader_format(r: fossil_base::NativeReader) -> SourceFormat {
    match r {
        fossil_base::NativeReader::CsvAuto => SourceFormat::Csv,
        fossil_base::NativeReader::JsonAuto => SourceFormat::Json,
        fossil_base::NativeReader::Parquet => SourceFormat::Parquet,
    }
}

/// Resolve the 1-based, MAPPING-RELATIVE source line of the `iri = ...`
/// property's RHS expression for the SC#4 named assertion (`line=<N>`).
///
/// # Why mapping-relative
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
        .position(|p| matches!(p.key, PropertyKey::Subject))
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
    db: &'db dyn fossil_base::Db,
    span_line: u32,
) -> Option<Expr<'db>> {
    let prop = body
        .properties(db)
        .iter()
        .find(|p| matches!(p.key, PropertyKey::Subject))?;
    Some(lower_property_value(
        db,
        &prop.value,
        &m.source_binding,
        Some(span_line),
    ))
}

/// Replace every [`HirExpr::Edge`] with the target type's identity template,
/// this row's values in its holes.
///
/// `buyer = Person(User.email)` becomes exactly the interpolation the `Person`
/// mapping wrote for its own `@subject`, with `User.email` where its hole was.
/// The result is indistinguishable from a hand-written IRI template — which is
/// the point: `apply_output_shape` already classifies a predicate as an edge
/// when the SHAPE says its range is a shape, so the constructor needs no new op
/// and no new classification rule. What it needed was a per-row IRI value the
/// language could actually produce, which is the hole it fills.
///
/// # This is what replaced `subject_skeletons`
///
/// An edge used to be GUESSED: the skeleton of a property's template — every
/// per-row hole replaced by a `\u{1}` marker — was compared against the skeleton
/// of every mapping's subject in the file, and a match made it a foreign key.
/// The comparison existed only because the identity rule was repeated per
/// mapping. There is now one identity per TYPE — every mapping producing `T`
/// declares the same `@subject`, and two that disagree are a compile error —
/// so the guess becomes this lookup.
///
/// # Fan-out
///
/// `subject_templates` is FILE-keyed and read LAZILY — the `iter().any(...)`
/// guard means a mapping with no edge constructor never asks for it. That is
/// what keeps `MAX_PER_MAPPING_FAN_OUT` at 1 for every fixture that has none.
fn resolve_edges(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    value: &HirExpr,
) -> HirExpr {
    if !contains_edge(value) {
        return value.clone();
    }
    let templates = fossil_hir::identity::subject_templates(db, file);
    let dm = def_map(db, file);
    substitute_edges(db, dm, templates, value)
}

/// The column type of an operator expression, by the same two rules the checker
/// applies in `synth_binop` — comparison and connective are `Bool`; arithmetic
/// is the WIDER operand, except `/`, which is always `Float`.
///
/// It re-derives rather than reads because a `VProp` is built from the HIR and
/// the checker's per-expression types are keyed by `(mapping, ExprId)`, which
/// this walk does not carry. That duplication is real and is the reason both
/// sides cite each other: if they ever disagree, the corpus declares one type
/// and holds another, and nothing downstream compares them.
fn operator_ty<'db>(
    db: &'db dyn fossil_base::Db,
    e: &HirExpr,
    field_ty: &impl Fn(&str) -> Ty<'db>,
) -> Ty<'db> {
    use fossil_hir::{BinOp, UnOp};
    let prim = |p| Ty::new(db, TyKind::Primitive(p));
    let is_float = |t: Ty<'db>| matches!(t.kind(db), TyKind::Primitive(Primitive::Float));
    match e {
        HirExpr::BinOp { op, lhs, rhs } => match op {
            BinOp::Div => prim(Primitive::Float),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Rem => {
                let l = operator_ty(db, lhs, field_ty);
                let r = operator_ty(db, rhs, field_ty);
                if is_float(l) || is_float(r) {
                    prim(Primitive::Float)
                } else {
                    prim(Primitive::Integer)
                }
            }
            _ => prim(Primitive::Bool),
        },
        // `-x` is `x`'s type; `not x` is Bool.
        HirExpr::UnaryOp { op, operand } => match op {
            UnOp::Neg => operator_ty(db, operand, field_ty),
            UnOp::Not => prim(Primitive::Bool),
        },
        HirExpr::ColumnRef { column, .. } | HirExpr::FieldRef(column) => field_ty(column.as_str()),
        HirExpr::FloatLit(_) => prim(Primitive::Float),
        HirExpr::IntLit(_) => prim(Primitive::Integer),
        HirExpr::BoolLit(_) => prim(Primitive::Bool),
        // A row whose return is not a scalar is a VERB of the algebra, and no
        // verb reaches here: a pipeline is lifted out of `Call` by
        // `crate::lower`, so what is left in an expression position is a scalar
        // function. `String` is this walk's answer for anything it cannot type,
        // which is what an uncatalogued name already gets.
        HirExpr::Call { func, .. } => fossil_hir::stdlib::stdlib()
            .lookup(func.as_str())
            .and_then(|entry| entry.sig.ret.scalar())
            .map_or_else(|| prim(Primitive::String), |s| s.to_ty(db)),
        // A conditional's branches agree by construction, so either answers.
        HirExpr::Ternary { then, .. } => operator_ty(db, then, field_ty),
        HirExpr::NullLit
        | HirExpr::StringLit(_)
        | HirExpr::Interpolation(_)
        | HirExpr::Edge { .. } => prim(Primitive::String),
    }
}

/// Does this expression contain an edge constructor anywhere?
///
/// The guard that keeps [`resolve_edges`] from reading the file-keyed identity
/// table for a mapping that has no edge — see its fan-out note.
fn contains_edge(e: &HirExpr) -> bool {
    match e {
        HirExpr::Edge { .. } => true,
        HirExpr::Call { args, .. } => args.iter().any(contains_edge),
        HirExpr::BinOp { lhs, rhs, .. } => contains_edge(lhs) || contains_edge(rhs),
        HirExpr::UnaryOp { operand, .. } => contains_edge(operand),
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => contains_edge(cond) || contains_edge(then) || contains_edge(otherwise),
        HirExpr::Interpolation(parts) => parts.iter().any(|p| match p {
            InterpolationPart::Hole(h) => contains_edge(h),
            InterpolationPart::Text(_) => false,
        }),
        HirExpr::NullLit
        | HirExpr::StringLit(_)
        | HirExpr::IntLit(_)
        | HirExpr::FloatLit(_)
        | HirExpr::BoolLit(_)
        | HirExpr::FieldRef(_)
        | HirExpr::ColumnRef { .. } => false,
    }
}

/// The recursive half of [`resolve_edges`]. An edge it cannot resolve is left
/// as an `Edge`, and the caller turns that into an internal bug — the checker
/// has already refused every case where it can happen.
fn substitute_edges<'db>(
    db: &'db dyn fossil_base::Db,
    dm: DefMap<'db>,
    templates: fossil_hir::identity::SubjectTemplates<'db>,
    e: &HirExpr,
) -> HirExpr {
    let recur = |x: &HirExpr| substitute_edges(db, dm, templates, x);
    // No wildcard: a variant added to `HirExpr` is a compile error here, not a
    // sub-expression this walk silently stops recursing into.
    #[deny(clippy::wildcard_enum_match_arm)]
    match e {
        HirExpr::Edge { target, args } => {
            // The arguments are resolved first: an edge whose argument is itself
            // an edge is legal, and nesting is the only reason this recurses.
            let args: Vec<HirExpr> = args.iter().map(&recur).collect();
            dm.lookup_type(db, target.as_str())
                .and_then(|iri| templates.for_shape(db, iri.as_str()))
                .and_then(|t| t.fill(&args))
                .unwrap_or_else(|| HirExpr::Edge {
                    target: target.clone(),
                    args,
                })
        }
        HirExpr::Call { func, args } => HirExpr::Call {
            func: func.clone(),
            args: args.iter().map(&recur).collect(),
        },
        HirExpr::BinOp { op, lhs, rhs } => HirExpr::BinOp {
            op: *op,
            lhs: Box::new(recur(lhs)),
            rhs: Box::new(recur(rhs)),
        },
        HirExpr::UnaryOp { op, operand } => HirExpr::UnaryOp {
            op: *op,
            operand: Box::new(recur(operand)),
        },
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => HirExpr::Ternary {
            cond: Box::new(recur(cond)),
            then: Box::new(recur(then)),
            otherwise: Box::new(recur(otherwise)),
        },
        HirExpr::Interpolation(parts) => HirExpr::Interpolation(
            parts
                .iter()
                .map(|p| match p {
                    InterpolationPart::Text(t) => InterpolationPart::Text(t.clone()),
                    InterpolationPart::Hole(h) => InterpolationPart::Hole(recur(h)),
                })
                .collect(),
        ),
        // The leaves, named — the same seven `contains_edge` answers `false` for.
        HirExpr::FieldRef(_)
        | HirExpr::ColumnRef { .. }
        | HirExpr::StringLit(_)
        | HirExpr::NullLit
        | HirExpr::IntLit(_)
        | HirExpr::FloatLit(_)
        | HirExpr::BoolLit(_) => e.clone(),
    }
}

/// Lower a property RHS [`HirExpr`] to a typed [`Expr`]. [`HirExpr::ColumnRef`]
/// and [`HirExpr::FieldRef`] → [`Expr::ColRef`]; [`HirExpr::StringLit`] →
/// [`Expr::LitString`]; [`HirExpr::Interpolation`] → the concat-chain of its
/// literal runs and its holes.
///
/// `assert_line` is `Some(N)` when lowering the subject position (the
/// `@subject` property), `None` for object positions. When `Some`, each per-row
/// hole — `{User.id}` — is wrapped in
/// `Expr::Assert { name: "iri_template_unbound", span_line: N, .. }` (SC#4 —
/// the un-statically-dischargeable NULL-field check).
///
/// # CODEGEN-LOWERING-01
///
/// **A qualified column reference keeps its binding.** `User.email` lowers to
/// `ColRef { source: "User", column: "email" }`, and the backend qualifies the
/// relation it reads under that same name — which is what makes
/// `Node.label` and `Other.label` two columns after a self-join instead of one.
///
/// The rule used to be the opposite: emit `source: SmolStr::default()` and let
/// the backend supply the relation from the source URI. It was written against
/// a SQL backend that pasted the source verbatim, so `users.id` reached `DuckDB`
/// for a view aliased `hello` and the binder refused it — and the fix was to
/// stop emitting the binding rather than to stop pasting it. `fossil-df` does
/// not paste: it resolves a `ColRef` against a `DFSchema`, so the binding is a
/// name to RESOLVE, not text to emit, and the crash the old rule prevented
/// cannot happen. What the old rule cost instead was the only thing that says
/// which side of a join a column came from — `join_key` could not tell, and
/// refused every join whose two keys were spelled differently.
///
/// This is the same discard `HirSourceOp::Select` had (`85bb488`), at the other
/// site: a `ColumnRef { binding, column }` destructured to check and rebuilt
/// with the binding dropped.
///
/// [`HirExpr::FieldRef`] — the retired bare `.column` — has no binding to carry
/// and still emits an empty source, which the backend resolves unqualified.
fn lower_property_value<'db>(
    db: &'db dyn fossil_base::Db,
    value: &HirExpr,
    source_binding: &SmolStr,
    assert_line: Option<u32>,
) -> Expr<'db> {
    // Exhaustive, no wildcard: this is where «every `HirExpr` variant has a
    // lowering arm» is enforced, by `rustc` and not by anyone remembering.
    #[deny(clippy::wildcard_enum_match_arm)]
    match value {
        // CODEGEN-LOWERING-01.
        HirExpr::ColumnRef { binding, column } => Expr::ColRef {
            source: binding.clone(),
            column: column.clone(),
        },
        HirExpr::FieldRef(field) => Expr::ColRef {
            source: SmolStr::default(),
            column: field.clone(),
        },
        HirExpr::StringLit(s) => Expr::LitString(s.clone()),
        HirExpr::Interpolation(parts) => {
            lower_interpolation(db, parts, source_binding, assert_line)
        }
        // The result type comes from the same catalog entry the checker typed
        // this call against — the backend derives the column's datatype from
        // it, so a call is no less typed than a column reference.
        HirExpr::IntLit(v) => Expr::LitInt(*v),
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => Expr::Ternary {
            cond: Box::new(lower_property_value(db, cond, source_binding, assert_line)),
            then: Box::new(lower_property_value(db, then, source_binding, assert_line)),
            otherwise: Box::new(lower_property_value(
                db,
                otherwise,
                source_binding,
                assert_line,
            )),
            // The branch type is the conditional's; the checker proved they
            // agree. Without a source row neither branch types, and String is
            // what every other untyped property gets.
            ty: Ty::new(db, TyKind::Primitive(Primitive::String)),
        },
        // A comparison against `null` is `IS NULL`, and it is decided HERE
        // rather than in a backend because it is a fact about the algebra: in
        // SQL `x != NULL` is NULL and not true, so lowering the surface's one
        // spelling as an ordinary comparison produces a filter that keeps no
        // rows. Both sides are checked because `null == x` is the same
        // question.
        HirExpr::BinOp { op, lhs, rhs }
            if matches!(op, fossil_hir::BinOp::Eq | fossil_hir::BinOp::Ne)
                && matches!(**lhs, HirExpr::NullLit) != matches!(**rhs, HirExpr::NullLit) =>
        {
            let operand = if matches!(**lhs, HirExpr::NullLit) {
                rhs
            } else {
                lhs
            };
            Expr::IsNull {
                operand: Box::new(lower_property_value(
                    db,
                    operand,
                    source_binding,
                    assert_line,
                )),
                negated: matches!(op, fossil_hir::BinOp::Ne),
            }
        }
        HirExpr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op: *op,
            lhs: Box::new(lower_property_value(db, lhs, source_binding, assert_line)),
            rhs: Box::new(lower_property_value(db, rhs, source_binding, assert_line)),
            ty: Ty::new(db, TyKind::Primitive(Primitive::Bool)),
        },
        HirExpr::FloatLit(v) => Expr::LitFloat(*v),
        HirExpr::BoolLit(b) => Expr::LitBool(*b),
        // `null` as a VALUE cannot reach here: the checker's `Eq`/`Ne` arm is
        // the only place its type is comparable with anything, so a property
        // written from it is refused with «expected …, got Null». The arm is
        // that invariant written down, and it answers with the string literal
        // the rest of this walk uses for a form it cannot read.
        HirExpr::NullLit => Expr::LitString(SmolStr::default()),
        // The operand's type is the unary's for `-` and Bool for `not`; both
        // are what `operator_ty` computes, and it is reused rather than
        // re-derived so the two cannot drift.
        HirExpr::UnaryOp { op, operand } => Expr::UnaryOp {
            op: *op,
            operand: Box::new(lower_property_value(
                db,
                operand,
                source_binding,
                assert_line,
            )),
            ty: Ty::new(
                db,
                TyKind::Primitive(match op {
                    fossil_hir::UnOp::Not => Primitive::Bool,
                    fossil_hir::UnOp::Neg => Primitive::Float,
                }),
            ),
        },
        HirExpr::Call { func, args } => Expr::Call {
            func: func.clone(),
            args: args
                .iter()
                .map(|a| lower_property_value(db, a, source_binding, assert_line))
                .collect(),
            ty: call_result_ty(db, func),
        },
        // An edge reaches MIR as the INTERPOLATION it fills, never as itself:
        // `resolve_edges` runs over every property value before this function
        // sees one, and `lower_to_mir_pg` refuses the mapping if any edge
        // survived. This arm is that invariant written down.
        //
        // It emits rather than returning an empty string, and that is the whole
        // point of the arm: an edge silently lowered to `''` produces a corpus
        // whose `by_source.parquet` is EMPTY and whose every conformance check
        // — each of them a count of violations — passes vacuously. A wrong
        // answer that reads as a green test is the one failure this file cannot
        // afford.
        HirExpr::Edge { target, .. } => {
            let _eg = fossil_base::bug(
                db,
                fossil_base::Span::new(0, 0),
                format!(
                    "an edge to `{target}` reached the value lowering unresolved — \
                     `resolve_edges` runs before this and the checker refuses a mapping whose \
                     edge has no template"
                ),
            );
            Expr::LitString(SmolStr::default())
        }
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
        .and_then(|e| e.sig.ret.scalar())
        .map_or_else(
            || Ty::new(db, TyKind::Primitive(Primitive::String)),
            |s| s.to_ty(db),
        )
}

/// Lower an interpolated string to the concat-chain of its parts.
///
/// This replaced `lower_iri_template` + `lower_placeholder`, which took the
/// template's raw text and scanned it for `${`, hand-parsing each hole at
/// MIR-lowering time. Two things were wrong with that beyond the duplication.
/// Its default arm echoed an unrecognised hole back as literal text, so
/// anything that was not `.field` or `prefix:` reached the output unexamined —
/// a mini format language nobody type-checked. And the prefix table had to be
/// carried into MIR so a lowering could resolve `${ex:}`, which is a HIR
/// concern that MIR now no longer sees.
///
/// `assert_line` is `Some(N)` in the subject position, where a hole that is
/// NULL at runtime would produce a malformed IRI. v0.1 cannot discharge
/// non-nullness statically, so every per-row hole is conservatively wrapped in
/// a named runtime assertion (SC#4); codegen renders it as
/// `CASE WHEN <expr> IS NOT NULL THEN <expr> ELSE error(...) END`.
fn lower_interpolation<'db>(
    db: &'db dyn fossil_base::Db,
    parts: &[InterpolationPart],
    source_binding: &SmolStr,
    assert_line: Option<u32>,
) -> Expr<'db> {
    let lowered = parts
        .iter()
        .map(|part| match part {
            InterpolationPart::Text(t) => Expr::LitString(t.clone()),
            InterpolationPart::Hole(e) => {
                let value = lower_property_value(db, e, source_binding, None);
                match (assert_line, is_per_row(e)) {
                    (Some(line), true) => Expr::Assert {
                        name: SmolStr::new_static("iri_template_unbound"),
                        span_line: line,
                        inner: Box::new(value),
                    },
                    _ => value,
                }
            }
        })
        .collect();
    fold_concat_left(lowered)
}

/// Whether an expression's value can differ from row to row — the question the
/// old text scan answered by looking for a leading `.`, which called
/// `clean.slug(.name)` constant.
fn is_per_row(e: &HirExpr) -> bool {
    match e {
        HirExpr::FieldRef(_) | HirExpr::ColumnRef { .. } => true,
        // An edge is per-row exactly when its arguments are, and by the time
        // MIR walks a body every edge has become the interpolation it fills
        // (`resolve_edges`). This arm is the honest answer for the window
        // before that, not a live path.
        HirExpr::Call { args, .. } | HirExpr::Edge { args, .. } => args.iter().any(is_per_row),
        HirExpr::BinOp { lhs, rhs, .. } => is_per_row(lhs) || is_per_row(rhs),
        HirExpr::UnaryOp { operand, .. } => is_per_row(operand),
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => is_per_row(cond) || is_per_row(then) || is_per_row(otherwise),
        HirExpr::Interpolation(parts) => parts.iter().any(|p| match p {
            InterpolationPart::Text(_) => false,
            InterpolationPart::Hole(e) => is_per_row(e),
        }),
        // A literal is the same for every row, and `null` is a literal.
        HirExpr::NullLit
        | HirExpr::StringLit(_)
        | HirExpr::IntLit(_)
        | HirExpr::FloatLit(_)
        | HirExpr::BoolLit(_) => false,
    }
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
    // Every `@subject` below is an interpolated string, and `{Rows.id}` is
    // fossil's hole, not a Rust format argument. The lint reads the Rust
    // literal and cannot know that.
    #![allow(clippy::literal_string_with_formatting_args)]

    use super::*;
    use fossil_hir::def_map::def_map;
    use std::sync::Arc;

    /// STDL-06: a mapping reading from an `io.json("...")` / `io.parquet("...")`
    /// binding lowers `Op::Source` with the real URI from the binding and the
    /// format selected by the constructor name.
    fn lower_source_for(src: &str) -> (SmolStr, SourceFormat) {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
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
    fn a_mapping_over_a_pipeline_reads_the_last_op_of_the_chain() {
        let src = "\
Users := io.csv(\"u.csv\")
Adults := Users.where(Users.edad >= 18)

People : Person from Adults
    @subject = \"https://example.org/user/{Users.id}\"
    name = Users.name
";
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        let ops = mir.ops(&db);

        assert!(
            matches!(&ops[0], Op::Source { binding, uri, .. }
                if binding.as_str() == "Users" && uri.as_str() == "u.csv"),
            "index 0 is the base source, got {:?}",
            ops[0]
        );
        let Op::Filter { input, pred } = &ops[1] else {
            panic!("index 1 is the `where`, got {:?}", ops[1]);
        };
        assert_eq!(*input, 0, "the filter reads the source");
        // `Users.edad` reaches MIR QUALIFIED — CODEGEN-LOWERING-01. The
        // qualification the surface requires is not spent by the lowering: the
        // backend qualifies the relation it reads under the same binding, and
        // after a join it is the only thing that says which side a column is.
        assert!(
            matches!(pred, Expr::BinOp { op: fossil_hir::BinOp::Ge, lhs, .. }
                if matches!(&**lhs, Expr::ColRef { source, column }
                    if source.as_str() == "Users" && column.as_str() == "edad")),
            "the predicate reads `Users.edad` under its binding, got {pred:?}"
        );
        assert!(
            matches!(&ops[2], Op::EmitVertex { input, .. } if *input == 1),
            "the vertex reads the FILTER, not the source, got {:?}",
            ops[2]
        );
    }

    /// A join lowers to `Op::Join` with both sides sourced, `Inner`, and a
    /// condition that is an EQUALITY between key columns — never an arbitrary
    /// boolean, so the class of plan stays an equi-join and the checker keeps
    /// the property that the condition names keys.
    ///
    /// The fixture wrote the retired `on = .k` sugar, where the MIR lowering
    /// SYNTHESISED the equality and put a relation name on each side. The
    /// author writes the equality now — `on = Orders.persona_id ==
    /// People.persona_id`, both sides qualified — so there is nothing left to
    /// synthesise and the condition lowers like any other expression, KEEPING
    /// the qualification (CODEGEN-LOWERING-01). It used to spend it, and that
    /// discard is what made `fossil_df::plan` refuse every join whose two keys
    /// were spelled differently.
    #[test]
    fn a_join_lowers_to_an_inner_equi_join_on_one_name() {
        let src = "\
Orders := io.csv(\"o.csv\")
People := io.csv(\"p.csv\")
Sales := Orders.join(People, on = Orders.persona_id == People.persona_id)

Venta : Person from Sales
    @subject = \"https://example.org/venta/{Orders.id}\"
    name = People.nombre
";
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        let ops = mir.ops(&db);

        assert!(matches!(&ops[0], Op::Source { binding, .. } if binding.as_str() == "Orders"));
        assert!(matches!(&ops[1], Op::Source { binding, .. } if binding.as_str() == "People"));
        let Op::Join {
            left,
            right,
            on,
            kind,
        } = &ops[2]
        else {
            panic!("index 2 is the join, got {:?}", ops[2]);
        };
        assert_eq!((left.input, right.input), (0, 1));
        assert_eq!(*kind, crate::op::JoinKind::Inner);
        // Each side's index travels WITH the name its columns are addressed by,
        // and neither side was written `as` anything.
        assert_eq!(left.names(), ["Orders"]);
        assert_eq!(right.names(), ["People"]);
        assert_eq!((&left.alias, &right.alias), (&None, &None));
        let Expr::BinOp {
            op: fossil_hir::BinOp::Eq,
            lhs,
            rhs,
            ..
        } = on
        else {
            panic!("the condition is one equality, got {on:?}");
        };
        assert!(
            matches!(&**lhs, Expr::ColRef { source, column }
                if source.as_str() == "Orders" && column.as_str() == "persona_id"),
            "got {lhs:?}"
        );
        assert!(
            matches!(&**rhs, Expr::ColRef { source, column }
                if source.as_str() == "People" && column.as_str() == "persona_id"),
            "the two sides are told apart by their bindings, not by their column \
             names — which here happen to agree, got {rhs:?}"
        );
        assert!(matches!(&ops[3], Op::EmitVertex { input, .. } if *input == 2));
    }

    /// **A join over a join is addressed by both its sides' bindings, and a
    /// pipeline by the ones its verbs left.** The two halves of one mistake:
    /// this carried ONE name, assigned `pipe.name` after a join and again at the
    /// end of every chain, and that name qualifies no column — `Op::Join` is the
    /// composite operator that re-qualifies neither side, and `Op::Filter`
    /// renames nothing.
    ///
    /// So `fossil check` admitted two programs the engine then refused, each
    /// naming the one qualifier that could not resolve as the alternative:
    /// `Both.join(Region, on = User.region_id == Region.id)` wanted `Both`, and
    /// `Purchase.join(Adults, on = … == User.id)` wanted `Adults`.
    /// `apps/docs/programs/chained-join` is both of them, executed.
    #[test]
    fn a_side_carries_every_binding_its_pipeline_left_addressable() {
        let src = "\
Purchase := io.csv(\"o.csv\")
User := io.csv(\"u.csv\")
Region := io.csv(\"r.csv\")
Adults := User.where(User.age >= 18)
Both := Purchase.join(Adults, on = Purchase.user_id == User.id)
Tri := Both.join(Region, on = User.region_id == Region.id)

Orders : Person from Tri
    @subject = \"https://example.org/order/{Purchase.id}\"
    name = Region.label
";
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        let ops = mir.ops(&db);

        let joins: Vec<_> = ops
            .iter()
            .filter_map(|o| match o {
                Op::Join { left, right, .. } => Some((left, right)),
                _ => None,
            })
            .collect();
        assert_eq!(joins.len(), 2, "two joins, got {ops:?}");

        // A filter renames nothing, so `Adults` is still addressed as `User` —
        // the case `JoinSide`'s own doc comment has described as working since
        // it was written, and which the end-of-chain assignment broke.
        let (left, right) = joins[0];
        assert_eq!(left.names(), ["Purchase"]);
        assert_eq!(
            right.names(),
            ["User"],
            "`Adults := User.where(…)` is addressed by `User`, not by its own name"
        );

        // And the second join's left side answers to BOTH — which is what makes
        // `User.region_id` a column of it.
        let (left, right) = joins[1];
        assert_eq!(
            left.names(),
            ["Purchase", "User"],
            "a join leaves both sides addressable"
        );
        assert!(left.addresses("User") && !left.addresses("Both"));
        assert_eq!(right.names(), ["Region"]);
    }

    /// `Node.join(Node as Other, …)` — the alias reaches the MIR as an alias,
    /// not folded into the right side's name. The backend needs the difference:
    /// an alias means "re-qualify this relation", and its absence means "leave
    /// the qualification it already has alone".
    #[test]
    fn a_self_join_carries_its_alias_to_the_right_side() {
        let src = "\
Node := io.csv(\"n.csv\")
Pairs := Node.join(Node as Other, on = Node.parent == Other.id)

Cat : Person from Pairs
    @subject = \"https://example.org/cat/{Node.id}\"
    name = Other.label
";
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let mapping = *dm.mappings(&db).first().expect("one mapping");
        let mir = lower_to_mir_pg(&db, mapping);
        let ops = mir.ops(&db);

        let Op::Join {
            left, right, on, ..
        } = ops
            .iter()
            .find(|o| matches!(o, Op::Join { .. }))
            .expect("a join")
        else {
            unreachable!()
        };
        assert_eq!(left.names(), ["Node"]);
        assert_eq!(left.alias, None);
        assert_eq!(right.relations, ["Node"], "both sides read `Node`");
        // The alias REPLACES the relations it re-qualifies, so the right side
        // answers to `Other` alone — which is the whole apparatus for telling
        // the two halves of a self-join apart.
        assert_eq!(right.names(), ["Other"]);
        assert_eq!(
            right.alias.as_deref(),
            Some("Other"),
            "the alias is what tells them apart"
        );
        // Two DIFFERENT column names — the shape `join_key` refused for as long
        // as the binding was discarded.
        assert!(
            matches!(on, Expr::BinOp { lhs, rhs, .. }
                if matches!(&**lhs, Expr::ColRef { source, column }
                        if source.as_str() == "Node" && column.as_str() == "parent")
                    && matches!(&**rhs, Expr::ColRef { source, column }
                        if source.as_str() == "Other" && column.as_str() == "id")),
            "got {on:?}"
        );
    }

    #[test]
    fn lower_to_mir_resolves_json_source() {
        let src = "\
Rows := io.json(\"a.json\")

People : Person from Rows
    @subject = \"https://example.org/user/{Rows.id}\"
    name = Rows.name
";
        let (uri, format) = lower_source_for(src);
        assert_eq!(uri.as_str(), "a.json");
        assert_eq!(format, SourceFormat::Json);
    }

    #[test]
    fn lower_to_mir_resolves_parquet_source() {
        let src = "\
Rows := io.parquet(\"a.parquet\")

People : Person from Rows
    @subject = \"https://example.org/user/{Rows.id}\"
    name = Rows.name
";
        let (uri, format) = lower_source_for(src);
        assert_eq!(uri.as_str(), "a.parquet");
        assert_eq!(format, SourceFormat::Parquet);
    }

    /// A db for the lowering helpers, which need one to type a `Ternary`.
    fn test_db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    #[test]
    fn interpolation_lowering_handles_trailing_literal() {
        // `{.id}/profile` → Concat(ColRef(users.id), LitString("/profile"))
        let db = test_db();
        let parts = vec![
            InterpolationPart::Hole(HirExpr::FieldRef("id".into())),
            InterpolationPart::Text("/profile".into()),
        ];
        let binding = SmolStr::new_static("users");
        // `assert_line = None` → object-position lowering (no Assert wrapper).
        let lowered: Expr<'_> = lower_interpolation(&db, &parts, &binding, None);
        match lowered {
            Expr::Concat(l, r) => {
                assert!(matches!(l.as_ref(), Expr::ColRef { .. }));
                assert!(matches!(r.as_ref(), Expr::LitString(s) if s.as_str() == "/profile"));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn a_per_row_hole_wraps_in_a_named_assertion_when_subject_context() {
        // `assert_line = Some(N)` (the subject position) → the hole is wrapped
        // in `Assert { name: "iri_template_unbound", span_line: N }` (SC#4).
        // The assertion NAME is a fixed snake_case identifier —
        // never type text.
        let db = test_db();
        let parts = vec![
            InterpolationPart::Hole(HirExpr::FieldRef("id".into())),
            InterpolationPart::Text("/profile".into()),
        ];
        let binding = SmolStr::new_static("users");
        let lowered: Expr<'_> = lower_interpolation(&db, &parts, &binding, Some(3));
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
    fn a_constant_hole_takes_no_assertion_and_fuses_with_its_neighbours() {
        // A hole whose expression is constant — it was the resolved prefix, and
        // a string literal is the same thing without the CURIE. What MIR owes is
        // the fusion the codegen's snapshots depend on: one literal, not three
        // concatenated.
        let db = test_db();
        let parts = vec![
            InterpolationPart::Hole(HirExpr::StringLit("https://example.org/".into())),
            InterpolationPart::Text("user/".into()),
            InterpolationPart::Hole(HirExpr::FieldRef("id".into())),
        ];
        let binding = SmolStr::new_static("users");
        // Subject position: the constant hole must NOT get an assertion, only
        // the per-row one would.
        let lowered: Expr<'_> = lower_interpolation(&db, &parts, &binding, Some(3));
        match lowered {
            Expr::Concat(l, r) => {
                assert!(
                    matches!(l.as_ref(), Expr::LitString(s) if s.as_str() == "https://example.org/user/"),
                    "expected fused prefix+literal, got {:?}",
                    l.as_ref()
                );
                assert!(
                    matches!(r.as_ref(), Expr::Assert { inner, .. }
                        if matches!(inner.as_ref(), Expr::ColRef { column, .. } if column.as_str() == "id")),
                    "got {:?}",
                    r.as_ref()
                );
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod null_lowering_tests {
    #![allow(clippy::literal_string_with_formatting_args)]

    use super::*;
    use fossil_hir::def_map::def_map;
    use std::sync::Arc;

    /// The one comparison whose SQL is not its spelling.
    ///
    /// `x != NULL` is NULL in SQL, not true, so lowering the surface's
    /// `x != null` as an ordinary `BinOp` gives a filter that keeps NO ROWS —
    /// and a filter that keeps nothing looks exactly like a filter that works
    /// when the fixture is small. The operator is decided in MIR because it is
    /// a fact about the algebra, not about a backend.
    fn filter_of(condition: &str) -> Expr<'static> {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db: &'static fossil_base::FossilDb =
            Box::leak(Box::new(fossil_base::FossilDb::new(system)));
        let src = format!(
            "Row := io.csv(\"r.csv\")\nValid := Row.where({condition})\nM : T from Valid\n    @subject = \"http://example.org/{{Row.id}}\"\n"
        );
        let file = fossil_base::SourceFile::new(db, src, "n.fossil".to_string());
        let dm = def_map(db, file);
        let mapping = *dm.mappings(db).first().expect("one mapping");
        let mir = lower_to_mir_pg(db, mapping);
        mir.ops(db)
            .iter()
            .find_map(|op| match op {
                Op::Filter { pred, .. } => Some(pred.clone()),
                _ => None,
            })
            .expect("the `where` lowered to a Filter")
    }

    #[test]
    fn a_comparison_against_null_lowers_to_is_null() {
        assert!(
            matches!(
                filter_of("Row.k == null"),
                Expr::IsNull { negated: false, .. }
            ),
            "`== null` is `IS NULL`"
        );
        assert!(
            matches!(
                filter_of("Row.k != null"),
                Expr::IsNull { negated: true, .. }
            ),
            "`!= null` is `IS NOT NULL`, and NOT `!= NULL`, which is NULL"
        );
        assert!(
            matches!(
                filter_of("null == Row.k"),
                Expr::IsNull { negated: false, .. }
            ),
            "`null == x` is the same question written the other way round"
        );
        assert!(
            matches!(filter_of("Row.k == \"a\""), Expr::BinOp { .. }),
            "a comparison against a value stays a comparison"
        );
    }
}
