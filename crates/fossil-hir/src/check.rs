//! Bidirectional type checker — Phase 3 (CORE-04/05/06) graduates Phase 2's
//! stub into the real `synth`/`check`/`compatible` algorithm.
//!
//! RESEARCH.md §"Bidirectional Checker Shape" carries the design. The
//! architectural keystone: [`typecheck_mapping`] is the ONLY new Salsa-tracked
//! entry per mapping. `synth` / `check` / `check_property` / `lookup_field` /
//! `compatible` are plain-Rust helpers called from inside it — this preserves
//! Phase 2's `MAX_PER_MAPPING_FAN_OUT = 1` invariant (LOAD-BEARING).
//!
//! Phase 2's [`crate::provenance::expr_types`] becomes a thin accessor over
//! [`typecheck_mapping`]'s output — Phase 3 inverts the dependency direction:
//! `typecheck_mapping` is now the source of truth for per-expression types;
//! `expr_types` is the projection.
//!
//! # Forward propagation (CSVW)
//!
//! When a mapping's source declares a CSVW `schema`,
//! [`crate::infer::resolve_source_row`] builds a `Record` type for the source
//! row; `.field` accesses resolve against it (SC#1, with did-you-mean on a
//! miss).
//!
//! # Backward checking
//!
//! When a target shape is resolved (see [`crate::shapes`]), `check_property`
//! matches each property's predicate against the shape's constraint table and
//! runs [`compatible`] against the type it declares; a predicate the shape
//! REQUIRES and the body never wrote is the other direction, and
//! [`Checker::check_required_properties`] is where the declared cardinality is
//! read. The shape is a decoded document, not a schema language: nothing here
//! names one.
//!
//! # No silent coercion (P-CRIT-4)
//!
//! Every type mismatch emits a [`Diagnostic`] via [`delay_span_bug`] AND
//! produces an [`ErrorGuaranteed`]. `typecheck_mapping` returns `Err` when the
//! body has even one type error.

use fossil_base::{Diagnostic, ErrorGuaranteed, Severity, Span, delay_span_bug};
use salsa::Accumulator;
use smol_str::SmolStr;

use crate::body::{ExprId, body};
use crate::def_map::MappingLoc;
use crate::didyoumean::did_you_mean;
use crate::infer::resolve_source_scope;
use crate::lower::{
    CmpOp, HirExpr, HirProperty, InterpolationPart, PropertyKey, UnOp, lower_to_hir,
};
use crate::provenance::{ExprTypeEntry, ExprTypes, Provenance, ProvenanceKind};
use crate::shapes::{NameCollision, ResolvedShape, TargetShapeError, resolve_target_shape};
use crate::spans::{Spans, mapping_header_span, spans};
use crate::ty::display::render_ty_kind;
use fossil_graph_schema::{Primitive, Rejection, local_name};

use crate::ty::{ShapeId, Ty, TyKind};

/// Which side of a two-span blame a position refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlamePos {
    /// Blame a body expression (resolved to a real span via [`Spans`]).
    Expr(ExprId),
    /// Blame a shape property constraint — Phase 3 v0.1 has no per-constraint
    /// span source, so the emitter falls back to the mapping-header span.
    ShapeProperty { shape: ShapeId, property: SmolStr },
}

/// Per-mapping type-check output. The source of truth for per-expression types
/// (Phase 3 inverts the Phase 2 `expr_types`-is-source-of-truth direction).
#[salsa::tracked(debug)]
pub struct TypeckOutput<'db> {
    pub expr_types: ExprTypes<'db>,
    pub source_row: Option<Ty<'db>>,
    pub target_shape: Option<ShapeId>,
    /// The target shape's predicates by the short name a body writes —
    /// `("name", "http://xmlns.com/foaf/0.1/name")` — in declaration order.
    ///
    /// **This is how `fossil-mir` gets its IRI back.** It used to strip one out
    /// of `PropertyKey::PrefixedName`, which the CURIE put there; a bare key
    /// severs that supply and the document is the only thing that knows. MIR
    /// already reads `typecheck_mapping` for the source row, so the IRI arrives
    /// through a seam that exists, as a pair of strings — no shape vocabulary,
    /// no descriptor, and nothing of what `0e6898d` cut comes back.
    #[returns(ref)]
    pub predicates: Vec<(SmolStr, SmolStr)>,
}

/// The ONE Salsa-tracked checker entry per mapping.
///
/// Reads `body` + `spans` + the source row (CSVW) + the target shape,
/// runs the plain-Rust bidirectional checker, and returns a [`TypeckOutput`].
/// Returns `Err(ErrorGuaranteed)` if the body has any type error (every error
/// also pushes ≥1 [`Diagnostic`] to the accumulator — P-CRIT-4).
#[salsa::tracked]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Phase 2-9 contract
pub fn typecheck_mapping<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Result<TypeckOutput<'db>, ErrorGuaranteed> {
    let hir_body = body(db, mapping);
    let spans_table = spans(db, mapping);
    // A source pipeline whose row algebra does not add up taints the mapping and
    // stops here. Checking the body against a row that could not be
    // built would report a second, invented error for every property that reads a
    // column the join was supposed to bring.
    let source_scope = resolve_source_scope(db, mapping)?;
    let source_row = source_scope.as_ref().and_then(|s| s.flat(db));
    // This used to pass `ACCEPT_ALL_DEFAULT` — one literal that turned backward
    // checking off for every program compiled through the checker, because the
    // in-query path had no descriptor to thread and the argument demanded one.
    // `resolve_target_shape` now reads the document the PROGRAM names, as a
    // Salsa input, so editing that document re-runs this query.
    //
    // The per-mapping fan-out is unchanged: the two queries it adds
    // (`file_at`'s registry read and `shape_document`) are keyed by the
    // DOCUMENT, not by the mapping, so ten mappings checking against one
    // document share one decode. `tests/invalidation_regression.rs` carries the
    // full accounting.
    let resolved_shape = match resolve_target_shape(db, mapping) {
        Ok(shape) => shape,
        Err(e) => {
            surface_target_shape_error(db, mapping, &e);
            None
        }
    };
    let target_shape_id = resolved_shape.as_ref().map(|r| r.shape_id);

    // The short-name table, and the collisions that make some names unwritable.
    // Built once per mapping: the body resolves every key against it, and a
    // collision is reported here rather than once per property that trips on it.
    //
    // The rename table comes off the `type` binding that introduced this
    // mapping's shape, keyed by the shape IRI because that is what survives the
    // header's resolution. It is read through `def_map`, which is file-keyed and
    // structurally stable across body edits, so it adds no per-mapping fan-out.
    let renames = shape_iri_of(db, mapping)
        .map(|iri| crate::def_map::def_map(db, mapping.file(db)).renames_for_shape(db, &iri))
        .unwrap_or_default();
    let (predicates, collisions) = resolved_shape
        .as_ref()
        .map_or_else(|| (Vec::new(), Vec::new()), |s| s.short_names(&renames));
    let mut cx = Checker {
        db,
        mapping,
        source_row,
        source_scope,
        resolved_shape,
        predicates: predicates.clone(),
        spans: spans_table,
        entries: Vec::new(),
        next_inference: 0,
        first_error: None,
        iri_position: false,
    };

    // Surface what the decoder rejected (SC#4) before checking the body — they are
    // informational + suggestive (they do not error the mapping out).
    cx.surface_shape_lowering_errors();
    cx.surface_name_collisions(&collisions);

    // Check each property's RHS.
    let properties = hir_body.properties(db);
    for (i, prop) in properties.iter().enumerate() {
        let expr_id = ExprId(u32::try_from(i).unwrap_or(u32::MAX));
        cx.check_property(expr_id, prop);
    }
    cx.check_required_properties(properties);

    let first_error = cx.first_error;
    let expr_types = ExprTypes::new(db, cx.entries);
    first_error.map_or_else(
        || {
            Ok(TypeckOutput::new(
                db,
                expr_types,
                source_row,
                target_shape_id,
                predicates,
            ))
        },
        Err,
    )
}

/// Report a target shape the program named and the document could not supply.
///
/// Each of these was a silent `None` — the same answer as "the program names no
/// document", which was a decision then and went unreported. Naming no document
/// is an error now and has its own variant, so the one these were confused with
/// is [`TargetShapeError::Undeclared`]: a misspelt shape name turned backward
/// checking off and said nothing, and the split between the two is the whole
/// point of the enum.
///
/// Informational-with-teeth, like [`crate::infer`]'s treatment of a source
/// binding that resolved no shape: a `Diagnostic` is accumulated (so the CLI
/// and the LSP show it) but the mapping is not poisoned — its body is still
/// worth checking forward, and refusing to check it would report a second,
/// invented error for every property.
fn surface_target_shape_error<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    e: &TargetShapeError,
) {
    // The mapping's header — `User : ex:Persn from users` — is where every one
    // of these belongs: the shape name that did not resolve is written there,
    // and so is the absence of a document. It used to be `Span { start: 0, end:
    // 0 }`, which does not mean "no underline": the default frame is
    // `MappingRelative`, so `rebase_to_file` turns it into `base..base` and the
    // squiggle lands on the mapping's first byte — a plausible place and the
    // wrong one. The commonest case is a misspelt shape name (`ex:Persn`).
    let span = mapping_header_span(db, mapping);
    let message = match e {
        TargetShapeError::NoDocument => "this program names no shape document, so it cannot \
             write a property: a property key is the last segment of a predicate IRI that a \
             shape declares. Bring one in with `type { … } = io.shex(\"shop.shex\")`."
            .to_string(),
        TargetShapeError::Unregistered { document } => format!(
            "the shape document `{document}` is not there, so this mapping's \
             output contract cannot be checked"
        ),
        TargetShapeError::Undecodable { document } => format!(
            "nothing here reads `{document}` as a shape document, so this \
             mapping's output contract cannot be checked"
        ),
        TargetShapeError::Unparseable { document, cause } => {
            format!("the shape document `{document}` did not parse: {cause}")
        }
        TargetShapeError::Undeclared {
            document,
            shape,
            declared,
        } => {
            let candidates = declared.iter().map(smol_str::SmolStr::as_str);
            let suggestion = did_you_mean(shape, candidates);
            let tail = suggestion.map_or_else(
                || {
                    if declared.is_empty() {
                        " — it declares no shapes at all".to_string()
                    } else {
                        format!(
                            " — it declares {}",
                            declared
                                .iter()
                                .map(|d| format!("`{d}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                },
                |s| format!(" — did you mean `{s}`?"),
            );
            format!("`{document}` declares no shape `{shape}`{tail}")
        }
    };
    let _eg = delay_span_bug(db, span, message);
}

/// Render the "split into N mappings" suggestion for a value disjunction
/// (SC#4): one mapping per branch, named `{base}{n}`, with one property line
/// per predicate the branch constrains.
///
/// This lived in the `ShEx` decoder and walked the `OneOf` AST node, which a
/// `SuggestionSeed` cloned and carried through the whole compiler so the
/// emitter could walk it again. [`Rejection::Disjunction`] carries the branch
/// predicates instead — the only thing the rendering ever read out of that
/// node — so the suggestion is written where it is emitted, over strings.
///
/// A predicate renders as the BARE NAME a body writes — the last segment of
/// its IRI. It used to render as `<absolute-iri>`, which was the
/// right call while an absolute IRI was a property key; it is not one now, so
/// the suggestion would have emitted Fossil that does not parse. The suggestion
/// is generated code and it has to compile, which
/// `the_generated_split_suggestion_compiles` is there to prove.
/// The parameters are in EMISSION order — name, shape, source, subject — and
/// that is not cosmetic: the function this replaces took the IRI template
/// second and the shape fourth, so a call site could swap the shape and the
/// `from` clause and still compile, still parse, and still pass a test that
/// only counted mappings. One did, in this commit, before this reorder.
#[must_use]
pub fn render_split_suggestion(
    base_mapping_name: &str,
    base_shape_iri: &str,
    base_from_clause: &str,
    base_iri_template: &str,
    disjuncts: &[Vec<String>],
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (i, branch) in disjuncts.iter().enumerate() {
        let idx = i + 1;
        // `write!` into a `String` is infallible.
        let _ = write!(
            out,
            "{base_mapping_name}{idx} : {base_shape_iri} from {base_from_clause}\n    @subject = {base_iri_template}\n",
        );
        if branch.is_empty() {
            // A branch whose predicates the decoder could not name — a nested
            // disjunction, or a reference it did not resolve. The mapping is
            // still the right shape; the user has to fill the body in.
            out.push_str("    # TODO: this branch names no predicate — split it by hand\n");
        }
        for predicate in branch {
            let short = local_name(predicate);
            let _ = writeln!(out, "    {short} = .{short}");
        }
        out.push('\n');
    }
    out
}

/// Plain-Rust bidirectional checker state. NOT `#[salsa::tracked]` — lives only
/// for the duration of one [`typecheck_mapping`] call.
// `missing_debug_implementations`: `Checker` holds `&dyn fossil_base::Db`
// (the Salsa database trait object is not `Debug`) plus interned handles; a
// derived `Debug` is impossible and a hand-rolled one would add noise without
// value (the struct is a short-lived checker scratchpad, never logged).
#[allow(missing_debug_implementations)]
pub struct Checker<'db> {
    pub(crate) db: &'db dyn fossil_base::Db,
    pub(crate) mapping: MappingLoc<'db>,
    pub(crate) source_row: Option<Ty<'db>>,
    /// The rows the `from` clause puts in scope, each under the name of the
    /// binding that introduced it — [`crate::infer::RowScope`]. `source_row` is
    /// this flattened, and a QUALIFIED reference must not use it: flattening is
    /// what loses the binding.
    pub(crate) source_scope: Option<crate::infer::RowScope<'db>>,
    pub(crate) resolved_shape: Option<ResolvedShape<'db>>,
    /// The target shape's predicates by short name — what a bare property key
    /// resolves against.
    pub(crate) predicates: Vec<(SmolStr, SmolStr)>,
    pub(crate) spans: Spans<'db>,
    pub(crate) entries: Vec<ExprTypeEntry<'db>>,
    pub(crate) next_inference: u32,
    /// First type error encountered (if any) — propagated as the mapping's
    /// `ErrorGuaranteed`. Every error also pushes a diagnostic, so any
    /// `Some(eg)` here implies ≥1 emitted `Diagnostic`.
    pub(crate) first_error: Option<ErrorGuaranteed>,
    /// Whether the expression being synthesised sits where an IRI is wanted.
    ///
    /// It exists because an interpolation's type is not a property of the
    /// expression: `"…{u.id}"` yields an IRI where an IRI is wanted and a string
    /// anywhere else. That is the rule read literally — "there is no IRI
    /// template: there is interpolation" — and the alternative
    /// is a value position typed `IriTemplate`, which would fail against any
    /// shape that declares its predicate a string.
    ///
    /// **It was called `subject_position` and was set from `@subject` alone**,
    /// which made it exactly one position wide — so an interpolation in VALUE
    /// position was `String` unconditionally, and a predicate whose range is a
    /// shape (which [`crate::shapes::expected_value_ty`] types `Iri`) could not
    /// be satisfied by anything the language could write. The flag is now set
    /// from two places, both in [`Checker::check_property`]: the identity's
    /// key, and the EXPECTATION the resolved predicate carries. The second one
    /// is what makes this checker bidirectional in the position where it
    /// matters — «where it sits» is a question the expected type answers, not
    /// just the key.
    pub(crate) iri_position: bool,
}

impl<'db> Checker<'db> {
    const fn db(&self) -> &'db dyn fossil_base::Db {
        self.db
    }

    fn span_of(&self, expr_id: ExprId) -> Span {
        self.spans
            .get(self.db, expr_id)
            .unwrap_or(Span { start: 0, end: 0 })
    }

    /// The mapping's header span — where a diagnostic about the mapping AS A
    /// WHOLE belongs, as opposed to one about an expression in its body.
    ///
    /// Three emitters used `Span { start: 0, end: 0 }` for this and it is not
    /// «no span»: see [`mapping_header_span`].
    fn header_span(&self) -> Span {
        mapping_header_span(self.db, self.mapping)
    }

    const fn record_error(&mut self, eg: ErrorGuaranteed) {
        if self.first_error.is_none() {
            self.first_error = Some(eg);
        }
    }

    /// Mint a fresh inference id. Phase 3 v0.1 uses this only for the closure
    /// hook; the checker is otherwise fully directional over the leaf
    /// `HirExpr` forms.
    const fn fresh_inference(&mut self) -> crate::ty::InferenceId {
        let id = crate::ty::InferenceId(self.next_inference);
        self.next_inference += 1;
        id
    }
}

/// Compatibility check: is `actual` a subtype of `expected` (per
/// type-system.md §9), AND does `actual`'s cardinality satisfy the constraint?
///
/// Phase 3 graduates Phase 2's pointer-equality stub to the subtyping rules
/// (S-Refl, S-IntFlt, S-TmplIri, S-SeqCov) + two-span blame with real spans
/// from [`Spans`]. S-Opt and S-OptCov went with `TyKind::Optional`, and the
/// cardinality check went with them — see the body.
///
/// `expected` is an `Option` because a shape document may decline to narrow a
/// predicate's value type at all (`ex:name .`), and `None` is that: the
/// cardinality still binds, the type does not. It used to arrive as
/// `TyKind::Iri` — the narrowest type in the lattice standing in for the
/// widest — so a constraint that constrained nothing rejected a `String`. See
/// [`crate::shapes::ShapeConstraint::value_ty`].
///
/// Plain-Rust (NOT `#[salsa::tracked]`). Called from within the tracked
/// [`typecheck_mapping`] frame (and from tests' tracked shims) so its
/// `delay_span_bug` emits are valid.
///
/// On mismatch, returns `Err(ErrorGuaranteed)` and pushes a two-span
/// diagnostic naming BOTH the source expression's span and the destination's.
pub fn compatible<'db>(
    cx: &mut Checker<'db>,
    actual: Ty<'db>,
    expected: Option<Ty<'db>>,
    source_expr: ExprId,
    dest: &BlamePos,
) -> Result<(), ErrorGuaranteed> {
    let db = cx.db();

    // Subtype check (`type-system.md` §9). A constraint that narrows nothing is
    // satisfied by anything.
    //
    // There used to be a second check here, and an `occurs: Occurs` parameter
    // to feed it: an `Optional<X>` value could not satisfy a constraint
    // demanding 1+. `TyKind::Optional` was never CONSTRUCTED anywhere but a
    // test — `expected_value_ty` emits `Primitive` or `Iri`, and
    // `record_from_descriptor` types every CSVW column bare — so the check
    // could not fire, and it is gone with the variant. The cardinality a shape
    // declares is enforced in exactly one place now, and it is the direction
    // that can be observed: [`Checker::check_required_properties`], over the
    // predicates the body never wrote.
    if expected.is_none_or(|e| subtypes(db, actual, e)) {
        return Ok(());
    }

    // Mismatch → two-span blame.
    let source_span = cx.span_of(source_expr);
    let dest_span = match dest {
        BlamePos::Expr(eid) => cx.span_of(*eid),
        // No per-constraint span source in Phase 3 v0.1 — fall back to the
        // source expression's span (the mapping-relative location of the RHS).
        BlamePos::ShapeProperty { .. } => source_span,
    };

    let actual_display = render_ty_kind(db, actual.kind(db));
    // Only reachable with `Some(_)`: a constraint that narrows nothing cannot
    // fail the subtype check.
    let expected_display = expected.map_or_else(
        || "any value".to_string(),
        |e| render_ty_kind(db, e.kind(db)),
    );
    let msg = format!(
        "expected `{expected_display}`, got `{actual_display}` \
         (expected because of the constraint at {dest_span:?})"
    );

    let eg = delay_span_bug(db, source_span, msg);
    cx.record_error(eg);
    Err(eg)
}

// `expr_contains_free_field_refs`, `rewrite_field_refs_to_row_dot` and
// `render_leaf_expr_text` lived here, and all three existed for ONE caller:
// the implicit-closure fast-path in `Checker::check`. They are named as dead in
// `grammar.bnf`'s own tombstone for `FieldRef` — «existed ONLY because a
// predicate did not name its row» — and the walk they performed answers a
// question the language stopped asking: which sub-expressions read the
// anonymous current row. Every reference is qualified now (`User.name`), so a
// closure has nothing to capture implicitly.

/// The source spelling of a unary operator, for diagnostics.
const fn un_op_text(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "not",
    }
}

/// The source spelling of an operator, for diagnostics.
const fn op_text(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::And => "and",
        CmpOp::Or => "or",
        CmpOp::Add => "+",
        CmpOp::Sub => "-",
        CmpOp::Mul => "*",
        CmpOp::Div => "/",
        CmpOp::Rem => "%",
    }
}

/// Recursive subtyping per `type-system.md` §9 (5 rules + reflexivity).
/// Direct enum dispatch — NO `Box<dyn>`, NO trait objects.
fn subtypes<'db>(db: &'db dyn fossil_base::Db, actual: Ty<'db>, expected: Ty<'db>) -> bool {
    // S-Refl: every type subtypes itself (pointer equality after interning).
    if actual == expected {
        return true;
    }
    // An Error type is compatible with anything (taint already emitted a
    // diagnostic; do not cascade).
    if matches!(actual.kind(db), TyKind::Error(_)) || matches!(expected.kind(db), TyKind::Error(_))
    {
        return true;
    }
    // S-IntFlt and S-TmplIri both answer `true`, and they are not one arm: each
    // is a named subtyping rule with its own reason, and the reason is what the
    // arm above it carries. Merged, the two rules would share one comment and
    // neither would be findable by name.
    #[allow(clippy::match_same_arms)]
    match (actual.kind(db), expected.kind(db)) {
        // S-IntFlt: Integer <: Float.
        (TyKind::Primitive(Primitive::Integer), TyKind::Primitive(Primitive::Float)) => true,
        // S-TmplIri: IriTemplate <: Iri. An IRI template is an IRI with holes
        // in it, and the holes are filled per row before anything sees the
        // value — which is precisely what the identity slot does with one, and
        // `TyKind::IriTemplate` exists for no other reason. Without this rule
        // the type is a dead end: nothing consumes an `IriTemplate`, so the
        // only expression the language has for building an IRI per row could
        // satisfy no constraint that wants an IRI.
        (TyKind::IriTemplate, TyKind::Iri) => true,
        // S-SeqCov: Seq<τ> <: Seq<τ'> when τ <: τ'.
        (TyKind::Seq(a_inner), TyKind::Seq(e_inner)) => subtypes(db, *a_inner, *e_inner),
        _ => false,
    }
}

// S-Opt and S-OptCov were two arms here, and `is_optional` was the predicate
// the cardinality check asked. All three are gone with `TyKind::Optional`,
// which nothing outside a test ever constructed: the language has no `T?` (it
// is not in `grammar.bnf`), `expected_value_ty` emits `Primitive` or `Iri`, and
// `record_from_descriptor` types every CSVW column bare. A rule over a type
// that cannot exist is not a rule.

// `demands_one_or_more` lived here as a five-armed match over a cardinality
// enum that could hold two spellings of one fact (`Exact(3)` answered `true`
// where `Range { min: 3, max: Some(3) }` answered `false`). `Occurs` is a
// `(min, max)` pair and carries the answer as `Occurs::demands_one_or_more`.

impl<'db> Checker<'db> {
    /// Inference-mode descent over a leaf [`HirExpr`].
    ///
    /// `HirExpr` is recursive now (`Call` / `Ternary` / `BinOp` /
    /// `Interpolation` all carry sub-expressions), so `synth` dispatches into
    /// the arms below. It had one mode transition — the `synthesize_closure`
    /// hook — and that hook is deleted: there is no checking-mode entry left in
    /// this impl, because the only caller with an expectation to give is
    /// [`Self::check_property`], which applies it itself.
    pub fn synth(&mut self, expr_id: ExprId, e: &HirExpr) -> Option<Ty<'db>> {
        let (ty, kind) = self.synth_ty(expr_id, e)?;
        let span = self.span_of(expr_id);
        self.entries.push(ExprTypeEntry {
            expr_id,
            ty,
            provenance: Provenance { span, kind },
        });
        Some(ty)
    }

    /// The type and its provenance, with nothing recorded.
    ///
    /// [`Self::synth`] is this plus the arena entry. The split exists because a
    /// call's arguments share the call's `expr_id` — recording them would put
    /// several entries under one id and hover would read whichever came first.
    fn synth_ty(&mut self, expr_id: ExprId, e: &HirExpr) -> Option<(Ty<'db>, ProvenanceKind)> {
        let db = self.db;
        let out = match e {
            HirExpr::StringLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::String)),
                ProvenanceKind::Literal,
            ),
            // T-Interp: the holes are typed like the expressions they are, and
            // the interpolation's own type comes from where it sits — IRI under
            // `iri =`, a string anywhere else.
            HirExpr::Interpolation(parts) => {
                for part in parts {
                    if let InterpolationPart::Hole(e) = part {
                        self.synth_ty(expr_id, e);
                    }
                }
                let kind = if self.iri_position {
                    TyKind::IriTemplate
                } else {
                    TyKind::Primitive(Primitive::String)
                };
                (Ty::new(db, kind), ProvenanceKind::Literal)
            }
            // T-Column: the qualified spelling. Same resolution as T-Field,
            // plus the check the anonymous form could never make — that the
            // name on the left is a row this mapping actually reads. That check
            // is the point of qualifying.
            //
            // **It was an equality against the `from` name, and that contradicted
            // the grammar.** `SourceDef` states the consequence of the split
            // between the two names — *«a mapping body writes `User.name` and never
            // `Adults.name`, even when it draws `from Adults`»* — so the name on
            // the left is a BINDING and the name after `from` is a RELATION, and
            // the two coincide only when the relation is a binding that reads a
            // file. Nine of the twenty-three conformance programs are written as
            // the grammar says and were rejected by this equality.
            //
            // It is a lookup over the scope, and the scope is what a `join` puts
            // two rows in: `Purchase.amount` and `User.email` land on different
            // entries, so two columns called `id` are two columns and not one.
            HirExpr::ColumnRef { binding, column } => {
                let Some(row_scope) = self.source_scope.as_ref() else {
                    // No `from` clause resolved at all — the header did not
                    // lower. Whatever refused it has already spoken; a second
                    // message per column would bury it.
                    return None;
                };
                if !row_scope.has(binding) {
                    let source_name = self.source_binding_name();
                    let eg = delay_span_bug(
                        db,
                        self.span_of(expr_id),
                        format!(
                            "`{binding}.{column}` reads a row this mapping does not \
                             have; it maps `{source_name}`. Name that row, or bring \
                             `{binding}` in."
                        ),
                    );
                    self.record_error(eg);
                    return None;
                }
                let ty = self.lookup_column(expr_id, binding, column)?;
                (
                    ty,
                    ProvenanceKind::InputDescriptor {
                        // The BINDING, not the `from` name: the column comes off
                        // `Contact`, and `Reachable` is the relation that carries
                        // it. Hover shows where a value came from, and after a
                        // join the `from` name is not where any of them came from.
                        source_name: binding.clone(),
                        column: column.clone(),
                    },
                )
            }
            // T-Field: resolve against the source row (CSVW).
            HirExpr::FieldRef(name) => {
                let ty = self.lookup_field(expr_id, name)?;
                let source_name = self.source_binding_name();
                (
                    ty,
                    ProvenanceKind::InputDescriptor {
                        source_name,
                        column: name.clone(),
                    },
                )
            }
            // T-App: resolve the name in the stdlib catalog, check the
            // arguments against the declared parameter types, and take the
            // declared return type. Every failure here is a diagnostic and an
            // `Error` type — never a silently dropped property.
            HirExpr::Call { func, args } => (
                self.synth_call(expr_id, func, args),
                ProvenanceKind::FnResult { name: func.clone() },
            ),
            // T-Edge: `Person(User.email)` is an IRI — the identity of a
            // `Person`, built from this row. This is the ONLY per-row producer
            // of `TyKind::Iri` the language has, which is what makes the
            // `expected_value_ty` rule for a shape-ranged predicate satisfiable
            // at all (`shapes.rs`, case 2: «the only thing a reference can be is
            // an IRI»). Before it, that rule could be met by nothing.
            HirExpr::Edge { target, args } => (
                self.synth_edge(expr_id, target, args),
                ProvenanceKind::FnResult {
                    name: target.clone(),
                },
            ),
            HirExpr::IntLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Integer)),
                ProvenanceKind::Literal,
            ),
            HirExpr::FloatLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Float)),
                ProvenanceKind::Literal,
            ),
            HirExpr::BoolLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Bool)),
                ProvenanceKind::Literal,
            ),
            // T-Unary: `-` keeps the operand's numeric type, `not` is Bool to
            // Bool. Neither widens — see [`Self::synth_unary`].
            HirExpr::UnaryOp { op, operand } => (
                self.synth_unary(expr_id, *op, operand),
                ProvenanceKind::BinaryOp {
                    op: SmolStr::new_static(un_op_text(*op)),
                },
            ),
            // T-Comp / T-And: both sides must agree, and the result is Bool
            // whether or not the operands could be typed.
            HirExpr::BinOp { op, lhs, rhs } => (
                self.synth_binop(expr_id, *op, lhs, rhs),
                ProvenanceKind::BinaryOp {
                    op: SmolStr::new_static(op_text(*op)),
                },
            ),
            HirExpr::Ternary {
                cond,
                then,
                otherwise,
            } => (
                self.synth_ternary(expr_id, cond, then, otherwise),
                ProvenanceKind::BinaryOp {
                    op: SmolStr::new_static("?:"),
                },
            ),
        };
        Some(out)
    }

    // `Checker::check` — checking mode — lived here, and its five callers were
    // five tests. `typecheck_mapping` never called it: `check_property` runs its
    // own `synth` + `compatible` because it has to resolve the predicate BEFORE
    // the synth (an interpolation's type depends on what is expected of it), and
    // that ordering is what a generic `check(expected)` could not express. What
    // it carried that nothing else did was the implicit-closure fast-path, and
    // that entry is dead: it fired on `TyKind::Fn`, which nothing constructs
    // because the user declares no functions, over
    // `expr_contains_free_field_refs`, which asked which
    // sub-expressions read the anonymous row the language no longer has.

    /// Check one property: synth its RHS and, if the shape declares the
    /// predicate its key names, check against that constraint.
    ///
    /// The key is a bare name now, so there is a resolution step in front of the
    /// check that did not exist: `name` means the predicate of this shape whose
    /// IRI ends in `name`, and a name no predicate ends in is an error with a
    /// did-you-mean over the ones that do. That is not a lookup failure to
    /// swallow — under a mandatory shape document (ruling 3 of 2026-08-11) a
    /// key the shape does not declare is a property that would be written into
    /// a corpus nothing describes.
    pub fn check_property(&mut self, expr_id: ExprId, prop: &HirProperty) {
        // The identity is not a shape predicate: a `.shex` describes the
        // predicates of a node, and in RDF the subject IS the node. A key the
        // shape does not declare, and a mapping with no resolved shape at all,
        // both yield no expectation — each has already said so, or says so in
        // `resolve_predicate` below.
        //
        // This resolution happens BEFORE the synth, and that ordering is the
        // whole of the bidirectionality: an interpolation's type depends on
        // what is expected of it, so the expectation cannot be looked up after
        // the fact. It used to be, and the consequence was measurable — an
        // interpolated IRI in value position was `String`, unsatisfiable
        // against every predicate whose range is a shape.
        // `Subject` and the unguarded `Name(_)` both answer `None` and cannot be
        // merged: the guarded `Name(name) if self.resolved_shape.is_some()` arm
        // sits between them, and match arms are tried in order. One combined arm
        // would have to go first, and it would shadow the guard.
        #[allow(clippy::match_same_arms)]
        let expectation = match &prop.key {
            PropertyKey::Subject => None,
            PropertyKey::Name(name) if self.resolved_shape.is_some() => {
                self.resolve_predicate(expr_id, name).and_then(|pred| {
                    let shape = self.resolved_shape.as_ref()?;
                    let constraint = shape.constraint_for(pred.as_str())?;
                    // `value_ty` is passed through as an `Option`, and that IS
                    // the fix: it used to be
                    // `unwrap_or_else(|| Ty::new(db, TyKind::Iri))`, so a
                    // predicate the document declined to narrow demanded the
                    // narrowest type in the lattice and rejected every string
                    // in the corpus.
                    Some((
                        constraint.value_ty,
                        BlamePos::ShapeProperty {
                            shape: shape.shape_id,
                            property: pred.clone(),
                        },
                    ))
                })
            }
            PropertyKey::Name(_) => None,
        };

        // Always synth the RHS so its type is recorded in `entries` (provenance
        // / hover consume this even when there is no backward constraint).
        let db = self.db;
        let expects_iri = expectation
            .as_ref()
            .is_some_and(|(ty, _)| ty.is_some_and(|t| matches!(t.kind(db), TyKind::Iri)));
        self.iri_position = matches!(prop.key, PropertyKey::Subject) || expects_iri;
        let actual = self.synth(expr_id, &prop.value);
        self.iri_position = false;

        if let (Some(actual), Some((expected, dest))) = (actual, expectation) {
            // `constraint.occurs` is deliberately NOT read here. The count a
            // shape declares is checked in `check_required_properties`, over the
            // predicates the body never wrote — the only direction that can be
            // observed, now that no value type can carry «zero or one» in
            // itself.
            let _ = compatible(self, actual, expected, expr_id, &dest);
        }
    }

    /// The predicate IRI a bare key names, or a diagnostic saying it names none.
    fn resolve_predicate(&mut self, expr_id: ExprId, name: &SmolStr) -> Option<SmolStr> {
        if let Some((_, iri)) = self.predicates.iter().find(|(n, _)| n == name) {
            return Some(iri.clone());
        }
        let db = self.db;
        let candidates: Vec<&str> = self.predicates.iter().map(|(n, _)| n.as_str()).collect();
        let suggestion = did_you_mean(name.as_str(), candidates.iter().copied());
        let msg = suggestion.map_or_else(
            || {
                if candidates.is_empty() {
                    format!("the target shape declares no predicate, so there is no `{name}`")
                } else {
                    format!(
                        "the target shape declares no `{name}` — it declares {}",
                        candidates
                            .iter()
                            .map(|c| format!("`{c}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            },
            |s| format!("the target shape declares no `{name}` — did you mean `{s}`?"),
        );
        let eg = delay_span_bug(db, self.span_of(expr_id), msg);
        self.record_error(eg);
        None
    }

    /// Two predicates of the shape with the same short name make BOTH of them
    /// unwritable, and that is reported once per mapping rather than once per
    /// property that trips on it.
    fn surface_name_collisions(&mut self, collisions: &[NameCollision]) {
        let db = self.db;
        let header_span = self.header_span();
        // **The recommendation is code, and both placeholders had to go.**
        //
        // This line said `@rename(<Type>, "…" as <another_name>)` and neither
        // half was writable: `<Type>` and `<another_name>` are not identifiers,
        // and `@rename` had no production at all — `AT_ATTR` at top level fell
        // through to `bump_as_error`. So the one repair the compiler names for
        // the one error it refuses to fix itself could not be pasted, in three
        // independent ways.
        //
        // Now the type is the bound name this mapping targets, the alias comes
        // from the vocabulary that brought the predicate in (see
        // `shapes::suggested_alias` for why a SUGGESTION is not the derivation
        // ADR-0059 §8 bans), and the production exists.
        // `the_recommended_rename_parses_and_repairs_the_collision` feeds this
        // exact text back through the compiler and checks that the collision
        // goes away and the renamed key resolves.
        let type_name = self.target_type_name();
        for c in collisions {
            let NameCollision {
                name,
                first,
                second,
            } = c;
            let alias = crate::shapes::suggested_alias(second);
            let eg = delay_span_bug(
                db,
                header_span,
                format!(
                    "two predicates of the target shape are both called `{name}`: `{first}` and \
                     `{second}`. A short name is the last segment of the predicate IRI, and \
                     fossil never picks between two. Give one of them another name above the \
                     binding: @rename({type_name}, \"{second}\" as {alias})"
                ),
            );
            self.record_error(eg);
        }
    }

    /// Every predicate the shape requires and the body never wrote.
    ///
    /// The other direction of the backward check, and the one that was missing:
    /// `check_property` walks what the body HAS, so a body that simply omits a
    /// required predicate passed every check there is. A shape that says
    /// `shop:email xsd:string ;` — cardinality exactly one — is a promise the
    /// corpus makes to its readers, and a mapping that does not keep it writes
    /// a node that does not conform to the shape it declares.
    fn check_required_properties(&mut self, properties: &[HirProperty]) {
        let db = self.db;
        let header_span = self.header_span();
        let Some(shape) = self.resolved_shape.as_ref() else {
            return;
        };
        let missing: Vec<(SmolStr, SmolStr)> = shape
            .constraints
            .iter()
            .filter(|c| c.occurs.demands_one_or_more())
            .map(|c| {
                (
                    SmolStr::from(local_name(c.predicate.as_str())),
                    c.predicate.clone(),
                )
            })
            .filter(|(short, _)| {
                !properties
                    .iter()
                    .any(|p| matches!(&p.key, PropertyKey::Name(n) if n == short))
            })
            .collect();
        for (short, iri) in missing {
            let eg = delay_span_bug(
                db,
                header_span,
                format!(
                    "this mapping never writes `{short}`, and the shape it produces requires it \
                     (`{iri}`). Add `{short} = ` to the body."
                ),
            );
            self.record_error(eg);
        }
    }

    /// Resolve a `.field` access against the source row.
    ///
    /// Returns `None` (synthesising no type, preserving the Phase 2 behaviour)
    /// when there is no declared source schema — this keeps `hello.fossil`
    /// (which has a `.name` `FieldRef` and no CSVW schema) free of spurious
    /// errors (walking-skeleton invariant). When the source row IS declared but
    /// the column is missing, emits a did-you-mean diagnostic (SC#1) and
    /// returns an `Error` type.
    pub fn lookup_field(&mut self, expr_id: ExprId, name: &str) -> Option<Ty<'db>> {
        let db = self.db;
        let row = self.source_row?; // no CSVW schema → no forward propagation

        let TyKind::Record(rec) = row.kind(db) else {
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!("internal: source row is not a Record for field `{name}`"),
            );
            self.record_error(eg);
            return Some(Ty::new(db, TyKind::Error(eg)));
        };

        if let Some(field) = rec.fields(db).iter().find(|f| f.name == name) {
            return Some(field.ty);
        }

        // Miss → did-you-mean over the declared columns.
        let candidates: Vec<&str> = rec.fields(db).iter().map(|f| f.name.as_str()).collect();
        let suggestion = did_you_mean(name, candidates.iter().copied());
        let msg = suggestion.map_or_else(
            || format!("unknown column `{name}`"),
            |s| format!("unknown column `{name}` — did you mean `{s}`?"),
        );
        let eg = delay_span_bug(db, self.span_of(expr_id), msg);
        self.record_error(eg);
        Some(Ty::new(db, TyKind::Error(eg)))
    }

    /// Resolve `Contact.email` against the ROW `Contact` contributes, and never
    /// against the flattening of the whole scope.
    ///
    /// That distinction is the entire point of the qualified spelling, and it is
    /// the trap this function exists to avoid: after `Purchase.join(User, …)`
    /// the flat row holds two columns called `id`, and a lookup by bare name
    /// finds the first — so `User.id` resolving through [`Self::lookup_field`]
    /// would type against `Purchase.id` and compile a program that writes the
    /// other row's column. Silent, and worse than the refusal it replaces.
    ///
    /// Returns `None` — synthesising no type, exactly as [`Self::lookup_field`]
    /// does — when the binding is in scope but its source declares no schema.
    /// The caller has already established the binding IS in scope
    /// ([`crate::infer::RowScope::has`]); an absent row here means "no
    /// descriptor", which is not an error.
    fn lookup_column(&mut self, expr_id: ExprId, binding: &str, column: &str) -> Option<Ty<'db>> {
        let db = self.db;
        let row = self.source_scope.as_ref()?.row_of(binding)?;

        let TyKind::Record(rec) = row.kind(db) else {
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!("internal: the row `{binding}` contributes is not a Record"),
            );
            self.record_error(eg);
            return Some(Ty::new(db, TyKind::Error(eg)));
        };

        if let Some(field) = rec.fields(db).iter().find(|f| f.name == column) {
            return Some(field.ty);
        }

        // Miss → did-you-mean over THAT binding's columns. Over the whole scope
        // it would suggest a column of the other side of a join, which is a
        // suggestion that does not compile.
        let candidates: Vec<&str> = rec.fields(db).iter().map(|f| f.name.as_str()).collect();
        let suggestion = did_you_mean(column, candidates.iter().copied());
        let msg = suggestion.map_or_else(
            || format!("unknown column `{column}` on `{binding}`"),
            |s| format!("unknown column `{column}` on `{binding}` — did you mean `{s}`?"),
        );
        let eg = delay_span_bug(db, self.span_of(expr_id), msg);
        self.record_error(eg);
        Some(Ty::new(db, TyKind::Error(eg)))
    }

    /// T-Edge: type a `Person(User.email)` against the target type's identity.
    ///
    /// Always `Iri` — «the only thing a reference to another node can be is an
    /// IRI», which is the rule [`crate::shapes::expected_value_ty`] applies on
    /// the other side. The type is returned even when the constructor is wrong,
    /// because poisoning is done through `record_error` and returning `Error`
    /// here as well would report the same property twice.
    ///
    /// Three things can go wrong and each is a diagnostic with a real span:
    ///
    /// 1. **The name binds no shape.** `type { Person } := io.shex(…)` where the
    ///    document declares fewer shapes than the binding names — `def_map`
    ///    already knows why and carries it as a `ShapeBindError`.
    /// 2. **No mapping in this file produces that type**, so there is no
    ///    template to fill. It is NOT an error for an edge to POINT at a type
    ///    nothing emits — RDF is open-world and ruling 6 of `SURFACE-PLAN.md`
    ///    says so — but it is an error to CONSTRUCT one, because construction
    ///    needs the template.
    /// 3. **The arity is wrong**: the template has N holes and the call passed
    ///    M. Both numbers are named, and so is the mapping that declared the
    ///    template, because a reader who mis-counted needs to see the identity
    ///    they are constructing.
    fn synth_edge(&mut self, expr_id: ExprId, target: &SmolStr, args: &[HirExpr]) -> Ty<'db> {
        let db = self.db;
        let iri_ty = Ty::new(db, TyKind::Iri);
        // The arguments are ordinary expressions in THIS mapping's scope and are
        // typed as such — that is the whole content of «the argument is the
        // hole's finished value». They share the call's `expr_id` for the same
        // reason `synth_call`'s do.
        for a in args {
            self.synth_ty(expr_id, a);
        }

        let file = self.mapping.file(db);
        let Some(shape_iri) = crate::def_map::def_map(db, file).lookup_type(db, target.as_str())
        else {
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!(
                    "`{target}` names no shape, so there is no identity to build. A `type {{ … }} \
                     := io.shex(…)` binding introduces the name, and the Nth name takes the Nth \
                     shape the document declares."
                ),
            );
            self.record_error(eg);
            return iri_ty;
        };

        // Read lazily, and that is load-bearing: `subject_templates` is
        // file-keyed and depends on every mapping's body, so a mapping that
        // constructs no edge must not read it. See `crate::identity`.
        let Some(template) =
            crate::identity::subject_templates(db, file).for_shape(db, shape_iri.as_str())
        else {
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!(
                    "no mapping in this program writes a `{target}`, so `{target}(…)` has no \
                     identity template to build from. An edge may POINT at a type nothing here \
                     emits — RDF is open-world — but it is built from the `@subject` of the \
                     mapping that does emit it."
                ),
            );
            self.record_error(eg);
            return iri_ty;
        };

        let arity = template.arity();
        if args.len() != arity {
            let declared = template.mapping_name;
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!(
                    "`{target}` is built from {arity} value(s) and this passes {}. Its identity \
                     is declared by `{declared}`, whose `@subject` has {arity} hole(s); the Nth \
                     value fills the Nth hole, in order.",
                    args.len()
                ),
            );
            self.record_error(eg);
        }
        iri_ty
    }

    /// T-App: type a `clean.slug(.name)` against the stdlib catalog.
    ///
    /// Four things can go wrong and all four are diagnostics with an `Error`
    /// type, so the mapping is poisoned and the property is never written from
    /// a value nobody checked: the name is not catalogued (with a did-you-mean
    /// over the catalog), the arity is wrong, an argument does not type, or an
    /// argument's type is not a subtype of the declared parameter's.
    ///
    /// Arguments carry the CALL's `expr_id`: the body arena holds one entry per
    /// property value, so a sub-expression has no id of its own. That makes
    /// every diagnostic inside a call point at the whole call — imprecise, and
    /// honestly so; per-argument spans need the arena to hold sub-expressions.
    fn synth_call(&mut self, expr_id: ExprId, func: &SmolStr, args: &[HirExpr]) -> Ty<'db> {
        let db = self.db;
        let reg = crate::stdlib::stdlib();
        let Some(entry) = reg.lookup(func.as_str()) else {
            // **Dispatch by RECEIVER, not by string.** The name
            // is dotted, so the miss has a shape, and saying WHICH HALF is
            // wrong is the whole difference between this and a string compare:
            // an unknown receiver and an unknown member are different mistakes
            // and used to produce the same sentence.
            let msg = match func.split_once('.') {
                Some((head, member)) if !reg.is_catalogued_head(head) => {
                    let heads = crate::didyoumean::did_you_mean(
                        head,
                        reg.iter().filter_map(|e| e.name.split('.').next()),
                    );
                    heads.map_or_else(
                        || {
                            format!(
                                "`{head}` is not a namespace or a type fossil knows, so it has \
                                 no member `{member}`"
                            )
                        },
                        |s| format!("`{head}` has no members — did you mean `{s}.{member}`?"),
                    )
                }
                Some((head, member)) => {
                    // The receiver EXISTS and the member does not. Suggest from
                    // that receiver's members only — the whole point of
                    // `members_of` is that the candidate set is the receiver's,
                    // not the catalogue's.
                    let (recv, _) = crate::stdlib::split_receiver(func.as_str());
                    let siblings = reg.members_of(recv).map(|e| e.member.as_str());
                    crate::didyoumean::did_you_mean(member, siblings).map_or_else(
                        || format!("`{head}` has no member `{member}`"),
                        |s| {
                            format!(
                                "`{head}` has no member `{member}` — did you mean `{head}.{s}`?"
                            )
                        },
                    )
                }
                None => format!("unknown function `{func}`"),
            };
            let eg = delay_span_bug(db, self.span_of(expr_id), msg);
            self.record_error(eg);
            return Ty::new(db, TyKind::Error(eg));
        };

        let params = &entry.sig.params;
        if args.len() != params.len() {
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!(
                    "`{func}` takes {} argument{}, but {} {} given",
                    params.len(),
                    if params.len() == 1 { "" } else { "s" },
                    args.len(),
                    if args.len() == 1 { "was" } else { "were" },
                ),
            );
            self.record_error(eg);
            return Ty::new(db, TyKind::Error(eg));
        }

        for (i, (arg, param)) in args.iter().zip(params.iter()).enumerate() {
            let expected = param.ty.to_ty(db);
            // An argument with no type is not an error: without a source
            // descriptor a `.field` synthesises nothing, and the rest of the
            // checker lets that through rather than inventing one. An argument
            // it cannot see is an argument it cannot check — which is what
            // forward propagation is FOR, and F3 is what makes it always
            // present. What it must not do is claim a problem the program does
            // not have.
            let Some((actual, _)) = self.synth_ty(expr_id, arg) else {
                continue;
            };
            if matches!(actual.kind(db), TyKind::Error(_)) {
                return actual;
            }
            if !subtypes(db, actual, expected) {
                // Argument 0 of a `Receiver::Scalar` row IS the receiver — the
                // two spellings both put it there, `str.trim(x)` writing
                // it and `x.trim()` moving it. So a type error on it is a
                // statement about what the value IS, and saying "argument 1"
                // for `x.trim()` would name a position the author never wrote.
                let is_receiver =
                    i == 0 && matches!(entry.recv, crate::stdlib::Receiver::Scalar(_));
                let msg = if is_receiver {
                    format!(
                        "`{}` is a member of {}, and this is {}",
                        entry.member,
                        render_ty_kind(db, expected.kind(db)),
                        render_ty_kind(db, actual.kind(db)),
                    )
                } else {
                    format!(
                        "argument {} of `{func}` expects {}, but this is {}",
                        i + 1,
                        render_ty_kind(db, expected.kind(db)),
                        render_ty_kind(db, actual.kind(db)),
                    )
                };
                let eg = delay_span_bug(db, self.span_of(expr_id), msg);
                self.record_error(eg);
                return Ty::new(db, TyKind::Error(eg));
            }
        }

        entry.sig.ret.to_ty(db)
    }

    /// T-Comp (`type-system.md` §4.7): `Γ ⊢ a : τ`, `Γ ⊢ b : τ`, τ comparable
    /// ⟹ `Bool`. T-And/T-Or: both sides `Bool` ⟹ `Bool`.
    ///
    /// An operand the checker cannot type — a `.field` with no source
    /// descriptor — is not an error, for the same reason it is not one in a
    /// call: forward propagation is what would give it a type, and F3 is what
    /// makes it always present. For a comparison or a connective the result is
    /// `Bool` regardless, because the operator says so and the operands cannot
    /// change that. For ARITHMETIC it is not: the result type IS the operands',
    /// so an operand with no type leaves the result unknown rather than guessed,
    /// and `Float` is the answer only where the operator forces it.
    ///
    /// # T-Arith, and the two rules it is
    ///
    /// `+`, `-`, `*` and `%` take two numbers and give the WIDER of them —
    /// `Integer` when both are `Integer`, `Float` as soon as either is. That is
    /// `subtypes`'s existing S-IntFlt promotion applied to a result rather than
    /// to a check, so the language has one widening rule and not two.
    ///
    /// **`/` is always `Float`**, and this is a decision rather than a
    /// consequence. The two engines disagree about integer division —
    /// `DuckDB`'s `/` on two `BIGINT`s gives a `DOUBLE`, `DataFusion`'s
    /// `Operator::Divide` on two `Int64`s gives an `Int64` and TRUNCATES — so a
    /// result type read off the operands would mean `7 / 2` is `3` here and
    /// `3.5` there. Typing it `Float` picks the answer that loses nothing, and
    /// `fossil_df::render` casts the left operand so the engine agrees with the
    /// type instead of the type flattering the engine.
    fn synth_binop(&mut self, expr_id: ExprId, op: CmpOp, lhs: &HirExpr, rhs: &HirExpr) -> Ty<'db> {
        let db = self.db;
        let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
        let l = self.synth_ty(expr_id, lhs).map(|(t, _)| t);
        let r = self.synth_ty(expr_id, rhs).map(|(t, _)| t);

        for t in [l, r].into_iter().flatten() {
            if let TyKind::Error(_) = t.kind(db) {
                return t;
            }
        }

        match op {
            // T-And / T-Or: each side must BE Bool.
            CmpOp::And | CmpOp::Or => {
                for (side, ty) in [("left", l), ("right", r)] {
                    let Some(ty) = ty else { continue };
                    if !subtypes(db, ty, bool_ty) {
                        let eg = delay_span_bug(
                            db,
                            self.span_of(expr_id),
                            format!(
                                "the {side} side of `{}` must be Bool, but it is {}",
                                op_text(op),
                                render_ty_kind(db, ty.kind(db)),
                            ),
                        );
                        self.record_error(eg);
                        return Ty::new(db, TyKind::Error(eg));
                    }
                }
            }
            // T-Comp: the two sides must be comparable to each other. Integer
            // and Float compare (the promotion `subtypes` already encodes);
            // a string against a number does not, and that is the mistake
            // worth catching — it is the one a mapping actually makes.
            CmpOp::Eq | CmpOp::Ne | CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                if let (Some(l), Some(r)) = (l, r)
                    && !subtypes(db, l, r)
                    && !subtypes(db, r, l)
                {
                    let eg = delay_span_bug(
                        db,
                        self.span_of(expr_id),
                        format!(
                            "cannot compare {} with {} using `{}`",
                            render_ty_kind(db, l.kind(db)),
                            render_ty_kind(db, r.kind(db)),
                            op_text(op),
                        ),
                    );
                    self.record_error(eg);
                    return Ty::new(db, TyKind::Error(eg));
                }
            }
            // T-Arith. Unlike the two above, the RESULT is the operands' and not
            // the operator's, so this arm returns rather than falling through to
            // `bool_ty`.
            CmpOp::Add | CmpOp::Sub | CmpOp::Mul | CmpOp::Div | CmpOp::Rem => {
                return self.synth_arith(expr_id, op, l, r);
            }
        }
        bool_ty
    }

    /// The numeric half of [`Self::synth_binop`]. See its docs for the two
    /// rules; this is where they are applied.
    fn synth_arith(
        &mut self,
        expr_id: ExprId,
        op: CmpOp,
        l: Option<Ty<'db>>,
        r: Option<Ty<'db>>,
    ) -> Ty<'db> {
        let db = self.db;
        let float_ty = Ty::new(db, TyKind::Primitive(Primitive::Float));

        // Three answers and not two. An operand with NO type (no source
        // descriptor yet — not an error, per this function's caller) and an
        // operand of the WRONG type must not collapse into one `None`, because
        // the first leaves the result unknown and the second poisons it.
        let mut widest: Option<bool> = Some(false);
        let mut poisoned: Option<ErrorGuaranteed> = None;
        for (side, ty) in [("left", l), ("right", r)] {
            let Some(ty) = ty else {
                widest = None;
                continue;
            };
            match ty.kind(db) {
                TyKind::Primitive(Primitive::Float) => {
                    if let Some(w) = widest.as_mut() {
                        *w = true;
                    }
                }
                TyKind::Primitive(Primitive::Integer) => {}
                // The mistake this arm exists to catch, and `+` is where it is
                // made: `User.first + " " + User.last` is what someone writes on
                // the first day. It is not string concatenation — `str.concat`
                // is, and one idea does not get two spellings — so the message
                // names the row rather than only refusing.
                kind => {
                    let hint = if matches!(kind, TyKind::Primitive(Primitive::String))
                        && matches!(op, CmpOp::Add)
                    {
                        ". Two strings are joined with `str.concat`, not `+`"
                    } else {
                        ""
                    };
                    let eg = delay_span_bug(
                        db,
                        self.span_of(expr_id),
                        format!(
                            "the {side} side of `{}` must be a number, but it is {}{hint}",
                            op_text(op),
                            render_ty_kind(db, kind),
                        ),
                    );
                    self.record_error(eg);
                    poisoned = Some(eg);
                }
            }
        }
        if let Some(eg) = poisoned {
            return Ty::new(db, TyKind::Error(eg));
        }

        // `/` is Float whatever it is given — the one rule that is the
        // operator's rather than the operands'. See `synth_binop`.
        if matches!(op, CmpOp::Div) {
            return float_ty;
        }
        match widest {
            // Both operands typed: the wider of the two, which is S-IntFlt
            // applied to a result.
            Some(true) => float_ty,
            Some(false) => Ty::new(db, TyKind::Primitive(Primitive::Integer)),
            // One side has no type at all, so neither has the result. `Unknown`
            // and not an invented `Integer`: a guess here becomes an
            // `xsd:integer` column in the corpus, and being wrong about that is
            // worse than saying nothing — which the shape check catches on its
            // own, against the shape, which is the thing that actually knows.
            None => {
                let id = self.fresh_inference();
                Ty::new(db, TyKind::Unknown(id))
            }
        }
    }

    /// T-Unary: `not` is `Bool → Bool`; `-` keeps its operand's numeric type.
    ///
    /// Neither widens. `-` on an `Integer` is an `Integer` — the negation of a
    /// whole number is a whole number — and promoting it to `Float` would make
    /// `-Row.n` a different type from `Row.n` for no reason the author could see.
    fn synth_unary(&mut self, expr_id: ExprId, op: UnOp, operand: &HirExpr) -> Ty<'db> {
        let db = self.db;
        let Some(ty) = self.synth_ty(expr_id, operand).map(|(t, _)| t) else {
            // Untypeable operand: the operator still fixes what it CAN. `not`
            // says Bool whatever it is given; `-` cannot, because its result is
            // its operand's type.
            return match op {
                UnOp::Not => Ty::new(db, TyKind::Primitive(Primitive::Bool)),
                UnOp::Neg => {
                    let id = self.fresh_inference();
                    Ty::new(db, TyKind::Unknown(id))
                }
            };
        };
        if let TyKind::Error(_) = ty.kind(db) {
            return ty;
        }
        let ok = match op {
            UnOp::Not => matches!(ty.kind(db), TyKind::Primitive(Primitive::Bool)),
            UnOp::Neg => matches!(
                ty.kind(db),
                TyKind::Primitive(Primitive::Integer | Primitive::Float)
            ),
        };
        if !ok {
            let wanted = match op {
                UnOp::Not => "Bool",
                UnOp::Neg => "a number",
            };
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                format!(
                    "`{}` needs {wanted}, but it is given {}",
                    un_op_text(op),
                    render_ty_kind(db, ty.kind(db)),
                ),
            );
            self.record_error(eg);
            return Ty::new(db, TyKind::Error(eg));
        }
        ty
    }

    /// T-Tern (`type-system.md` §4.8): the condition is `Bool`, both branches
    /// have the same type, and that type is the conditional's. **No implicit
    /// coercion** — two branches of different types is the error, not a widening
    /// nobody asked for, because a column whose type depends on the row is a
    /// column no shape can check.
    ///
    /// A branch the checker cannot type yields to the other branch, and if
    /// neither can be typed the conditional cannot either: it is `Unknown`, not
    /// an invented `String`.
    fn synth_ternary(
        &mut self,
        expr_id: ExprId,
        cond: &HirExpr,
        then: &HirExpr,
        otherwise: &HirExpr,
    ) -> Ty<'db> {
        let db = self.db;
        let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));

        if let Some((c, _)) = self.synth_ty(expr_id, cond) {
            if let TyKind::Error(_) = c.kind(db) {
                return c;
            }
            if !subtypes(db, c, bool_ty) {
                let eg = delay_span_bug(
                    db,
                    self.span_of(expr_id),
                    format!(
                        "the condition of `? :` must be Bool, but it is {}",
                        render_ty_kind(db, c.kind(db)),
                    ),
                );
                self.record_error(eg);
                return Ty::new(db, TyKind::Error(eg));
            }
        }

        let t = self.synth_ty(expr_id, then).map(|(t, _)| t);
        let o = self.synth_ty(expr_id, otherwise).map(|(t, _)| t);
        for ty in [t, o].into_iter().flatten() {
            if let TyKind::Error(_) = ty.kind(db) {
                return ty;
            }
        }

        match (t, o) {
            (Some(t), Some(o)) => {
                // Same type, or one is a subtype of the other (Integer widens
                // into Float; an interpolation widens into an IRI).
                if subtypes(db, t, o) {
                    o
                } else if subtypes(db, o, t) {
                    t
                } else {
                    let eg = delay_span_bug(
                        db,
                        self.span_of(expr_id),
                        format!(
                            "the branches of `? :` have different types: {} and {}.                              Both branches must have the same type — fossil does not coerce.",
                            render_ty_kind(db, t.kind(db)),
                            render_ty_kind(db, o.kind(db)),
                        ),
                    );
                    self.record_error(eg);
                    Ty::new(db, TyKind::Error(eg))
                }
            }
            // One branch typed and the other not: the typed one is the best
            // evidence available, and it is evidence, not a guess.
            (Some(t), None) | (None, Some(t)) => t,
            (None, None) => Ty::new(db, TyKind::Unknown(self.fresh_inference())),
        }
    }

    // `synthesize_closure` lived here — the implicit closure was the ONLY lambda
    // form in the language, and it existed to give `.age >= 18` a row to read
    // `.age` from. `grammar.bnf` names it dead in the same breath as `FieldRef`:
    // a reference is qualified now (`User.age`), so it names its own row and
    // there is nothing left to bind. Its one caller was `Checker::check`, above,
    // and `ProvenanceKind::SynthesizedClosureRendering` is what remains of it —
    // nothing in the workspace constructs that variant any more.

    /// Surface what the decoder could not lower (a value disjunction → the SC#4
    /// split suggestion) as diagnostics on this mapping. The disjunction one is
    /// informational — it does NOT error the mapping out, because the body may
    /// still check the parts that DID lower.
    // `literal_string_with_formatting_args`: the placeholder Fossil IRI
    // template (`${ex:}item/${.id}`) passed to the suggestion renderer is
    // LITERAL Fossil source, not a Rust format string.
    #[allow(clippy::literal_string_with_formatting_args)]
    fn surface_shape_lowering_errors(&mut self) {
        let Some(shape) = self.resolved_shape.as_ref() else {
            return;
        };
        let db = self.db;
        let header_span = self.header_span();
        // Clone the data we need so we don't hold a borrow of `self` across the
        // mutable `record_error` calls.
        let rejections = shape.rejections.clone();
        let base_name = self.mapping_name();
        let source_name = self.source_binding_name();
        for rejection in &rejections {
            match rejection {
                Rejection::Disjunction {
                    shape_iri,
                    disjuncts,
                } => {
                    let suggestion = render_split_suggestion(
                        base_name.as_str(),
                        shape_iri,
                        // The `from` clause is the SOURCE BINDING, not the
                        // mapping. This argument was `base_name` — the mapping's
                        // own name — so the suggestion emitted
                        // `Contact1 : ex:Contact from Contact`, a `from` that
                        // points at the mapping being split. The docblock on
                        // `render_split_suggestion` warns about exactly this
                        // swap because it had already happened once; the
                        // parameter reorder it describes made the two arguments
                        // adjacent and did not stop them being the same string.
                        // No test saw it: the corpus passed `"users"` by hand.
                        source_name.as_str(),
                        // Phase 3 v0.1 has no structured access to the consuming
                        // mapping's iri-template / from-clause text here; pass
                        // placeholders the renderer fills with the branch
                        // predicates. (The corpus test in plan 03-08 pins the
                        // exact rendered text.)
                        "`${ex:}item/${.id}`",
                        disjuncts,
                    );
                    let n = disjuncts.len();
                    let msg = format!(
                        "a value disjunction is not supported in v0.1 ({n} \
                         branches in shape `{shape_iri}`); help: split into {n} \
                         separate mappings (one per branch). The \
                         split-into-mappings suggestion is provided \
                         programmatically (see `Diagnostic.suggestion_source`)."
                    );
                    // Structured suggestion carrier (Blocker #3) — NOT a
                    // Markdown delimiter. Accumulate directly (informational;
                    // no ErrorGuaranteed).
                    Diagnostic::new(Severity::Error, msg, header_span)
                        .with_suggestion_source(suggestion)
                        .accumulate(db);
                }
                Rejection::CyclicRef { path } => {
                    let eg = delay_span_bug(
                        db,
                        header_span,
                        format!("cyclic shape graph not supported: {}", path.join(" -> ")),
                    );
                    self.record_error(eg);
                }
                Rejection::UnresolvedRef { label, in_shape } => {
                    let eg = delay_span_bug(
                        db,
                        header_span,
                        format!("unresolved shape ref `{label}` in shape `{in_shape}`"),
                    );
                    self.record_error(eg);
                }
                Rejection::Malformed(m) => {
                    let eg =
                        delay_span_bug(db, header_span, format!("malformed shape document: {m}"));
                    self.record_error(eg);
                }
            }
        }
    }

    /// The mapping's name text (for diagnostics + suggestion generation).
    fn mapping_name(&self) -> SmolStr {
        let file = self.mapping.file(self.db);
        lower_to_hir(self.db, file)
            .mappings(self.db)
            .get(self.mapping.index(self.db))
            .map_or_else(|| SmolStr::from("Mapping"), |m| m.name.clone())
    }

    /// The mapping's source binding name (for provenance).
    fn source_binding_name(&self) -> SmolStr {
        let file = self.mapping.file(self.db);
        lower_to_hir(self.db, file)
            .mappings(self.db)
            .get(self.mapping.index(self.db))
            .map_or_else(|| SmolStr::from(""), |m| m.source_binding.clone())
    }

    /// The LOCAL name of the type this mapping targets — `Person`, the word a
    /// `type { Person } := …` binding introduced and the header repeats.
    ///
    /// Not the shape IRI: it is what a `@rename`'s first argument has to be, and
    /// a suggestion that quoted the IRI there would not compile.
    fn target_type_name(&self) -> SmolStr {
        let db = self.db;
        let file = self.mapping.file(db);
        let Some(iri) = shape_iri_of(db, self.mapping) else {
            return SmolStr::new_static("Type");
        };
        crate::def_map::def_map(db, file)
            .types(db)
            .iter()
            .find(|t| t.shape_iri.as_deref() == Some(iri.as_str()))
            .map_or_else(|| SmolStr::new_static("Type"), |t| t.name.clone())
    }
}

/// The fully-resolved shape IRI a mapping targets, or `None` when its header
/// did not lower.
///
/// A free function rather than a `Checker` method because `typecheck_mapping`
/// needs it BEFORE the `Checker` exists — the rename table is an input to the
/// short-name table, which is an input to the `Checker`.
fn shape_iri_of<'db>(db: &'db dyn fossil_base::Db, mapping: MappingLoc<'db>) -> Option<SmolStr> {
    let file = mapping.file(db);
    lower_to_hir(db, file)
        .mappings(db)
        .get(mapping.index(db))
        .map(|m| m.shape_iri.clone())
        .filter(|iri| !iri.is_empty())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "check_tests.rs"]
mod tests;
