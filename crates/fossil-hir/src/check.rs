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
//! # Backward checking (`ShEx`)
//!
//! When a `ShEx` target shape is resolved (see [`crate::shapes`]),
//! `check_property` matches each property's predicate against the shape's
//! constraint table and runs [`compatible`] with the declared cardinality.
//! Optional-vs-cardinality-1+ produces a two-span blame error (SC#2).
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
use crate::infer::resolve_source_row;
use crate::lower::{HirExpr, HirProperty, PropertyKey, lower_to_hir};
use crate::provenance::{ExprTypeEntry, ExprTypes, Provenance, ProvenanceKind};
use crate::shapes::{ResolvedShape, resolve_target_shape};
use crate::spans::{Spans, spans};
use crate::ty::display::render_ty_kind;
use crate::ty::{Primitive, ShapeId, Ty, TyKind};
use fossil_descriptors_output::Cardinality;

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
}

/// The ONE Salsa-tracked checker entry per mapping.
///
/// Reads `body` + `spans` + the source row (CSVW) + the target shape (`ShEx`),
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
    let source_row = resolve_source_row(db, mapping);
    let resolved_shape = resolve_target_shape(db, mapping);
    let target_shape_id = resolved_shape.as_ref().map(|r| r.shape_id);

    let mut cx = Checker {
        db,
        mapping,
        source_row,
        resolved_shape,
        spans: spans_table,
        entries: Vec::new(),
        next_inference: 0,
        first_error: None,
    };

    // Surface OneOf rejections (SC#4) before checking the body — they are
    // informational + suggestive (they do not error the mapping out).
    cx.surface_shape_lowering_errors();

    // Check each property's RHS.
    let properties = hir_body.properties(db);
    for (i, prop) in properties.iter().enumerate() {
        let expr_id = ExprId(u32::try_from(i).unwrap_or(u32::MAX));
        cx.check_property(expr_id, prop);
    }

    let first_error = cx.first_error;
    let expr_types = ExprTypes::new(db, cx.entries);
    first_error.map_or_else(
        || {
            Ok(TypeckOutput::new(
                db,
                expr_types,
                source_row,
                target_shape_id,
            ))
        },
        Err,
    )
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
    pub(crate) resolved_shape: Option<ResolvedShape<'db>>,
    pub(crate) spans: Spans<'db>,
    pub(crate) entries: Vec<ExprTypeEntry<'db>>,
    pub(crate) next_inference: u32,
    /// First type error encountered (if any) — propagated as the mapping's
    /// `ErrorGuaranteed`. Every error also pushes a diagnostic, so any
    /// `Some(eg)` here implies ≥1 emitted `Diagnostic`.
    pub(crate) first_error: Option<ErrorGuaranteed>,
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

    const fn record_error(&mut self, eg: ErrorGuaranteed) {
        if self.first_error.is_none() {
            self.first_error = Some(eg);
        }
    }

    /// Mint a fresh inference id. Phase 3 v0.1 uses this only for the closure
    /// hook; the checker is otherwise fully directional over the leaf
    /// `HirExpr` forms.
    #[allow(dead_code)]
    const fn fresh_inference(&mut self) -> crate::ty::InferenceId {
        let id = crate::ty::InferenceId(self.next_inference);
        self.next_inference += 1;
        id
    }
}

/// Compatibility check: is `actual` a subtype of `expected` (per
/// type-system.md §9), AND does `actual`'s cardinality satisfy the constraint?
///
/// Phase 3 graduates Phase 2's pointer-equality stub to the 5 subtyping rules
/// (S-Refl, S-Opt, S-OptCov, S-SeqCov, S-IntFlt) + cardinality enforcement +
/// two-span blame with real spans from [`Spans`].
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
    expected: Ty<'db>,
    cardinality: Cardinality,
    source_expr: ExprId,
    dest: &BlamePos,
) -> Result<(), ErrorGuaranteed> {
    let db = cx.db();

    // 1. Subtype check (`type-system.md` §9).
    let subtype_ok = subtypes(db, actual, expected);

    // 2. Cardinality check: an `Optional<X>` actual cannot satisfy a constraint
    //    that demands at least one value.
    let cardinality_violated = is_optional(db, actual) && demands_one_or_more(cardinality);

    if subtype_ok && !cardinality_violated {
        return Ok(());
    }

    // 3. Mismatch → two-span blame.
    let source_span = cx.span_of(source_expr);
    let dest_span = match dest {
        BlamePos::Expr(eid) => cx.span_of(*eid),
        // No per-constraint span source in Phase 3 v0.1 — fall back to the
        // source expression's span (the mapping-relative location of the RHS).
        BlamePos::ShapeProperty { .. } => source_span,
    };

    let actual_display = render_ty_kind(db, actual.kind(db));
    let expected_display = render_ty_kind(db, expected.kind(db));

    let msg = if cardinality_violated {
        format!(
            "`{actual_display}` provided where the target shape demands \
             cardinality 1+ (expected at least one `{expected_display}`) \
             (required at {dest_span:?})"
        )
    } else {
        format!(
            "expected `{expected_display}`, got `{actual_display}` \
             (expected because of the constraint at {dest_span:?})"
        )
    };

    let eg = delay_span_bug(db, source_span, msg);
    cx.record_error(eg);
    Err(eg)
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
    match (actual.kind(db), expected.kind(db)) {
        // S-IntFlt: Integer <: Float.
        (TyKind::Primitive(Primitive::Integer), TyKind::Primitive(Primitive::Float)) => true,
        // S-Opt / S-OptCov: τ <: Optional<τ'> when τ <: τ' (a bare value lifts
        // into an optional position; an Optional covariantly subtypes another
        // Optional via the same recursion since `actual` matches `_`).
        (_, TyKind::Optional(inner)) => subtypes(db, actual, *inner),
        // S-SeqCov: Seq<τ> <: Seq<τ'> when τ <: τ'.
        (TyKind::Seq(a_inner), TyKind::Seq(e_inner)) => subtypes(db, *a_inner, *e_inner),
        _ => false,
    }
}

/// Is `ty` an `Optional<_>`?
fn is_optional<'db>(db: &'db dyn fossil_base::Db, ty: Ty<'db>) -> bool {
    matches!(ty.kind(db), TyKind::Optional(_))
}

/// Does `cardinality` require at least one value?
const fn demands_one_or_more(cardinality: Cardinality) -> bool {
    match cardinality {
        Cardinality::Exact(n) => n >= 1,
        Cardinality::OneOrMore => true,
        Cardinality::ZeroOrOne | Cardinality::ZeroOrMore => false,
        Cardinality::Range { min, .. } => min >= 1,
    }
}

impl<'db> Checker<'db> {
    /// Inference-mode descent over a leaf [`HirExpr`].
    ///
    /// Phase 2's `HirExpr` is non-recursive (`Template` / `FieldRef` /
    /// `StringLit` / `PrefixedName` are all leaf forms), so `synth` is a flat
    /// dispatch. Recursive arms (`Call` / `Pipeline` / `Ternary` / `BinOp`)
    /// land when the Pratt-lowered expression tree extends `HirExpr` in a later
    /// phase; the `synthesize_closure` hook (plan 03-06) is the one mode
    /// transition this plan stubs.
    pub fn synth(&mut self, expr_id: ExprId, e: &HirExpr) -> Option<Ty<'db>> {
        let db = self.db;
        let (ty, kind) = match e {
            HirExpr::StringLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::String)),
                ProvenanceKind::Literal,
            ),
            // T-Template: a backtick template in IRI position yields IriTemplate
            // (Phase 3 v0.1 default — refined by the caller's check context).
            HirExpr::Template(_) => (Ty::new(db, TyKind::IriTemplate), ProvenanceKind::Literal),
            // T-PrefixedName: an IRI literal.
            HirExpr::PrefixedName { .. } => (Ty::new(db, TyKind::Iri), ProvenanceKind::Literal),
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
        };
        let span = self.span_of(expr_id);
        self.entries.push(ExprTypeEntry {
            expr_id,
            ty,
            provenance: Provenance { span, kind },
        });
        Some(ty)
    }

    /// Checking-mode entry. Phase 3 v0.1: synth then [`compatible`].
    pub fn check(
        &mut self,
        expr_id: ExprId,
        e: &HirExpr,
        expected: Ty<'db>,
        cardinality: Cardinality,
        dest: &BlamePos,
    ) -> Result<(), ErrorGuaranteed> {
        let Some(actual) = self.synth(expr_id, e) else {
            // synth already emitted a diagnostic + recorded the error (e.g. a
            // field-not-found). Propagate without double-reporting.
            return self.first_error.map_or(Ok(()), Err);
        };
        compatible(self, actual, expected, cardinality, expr_id, dest)
    }

    /// Check one property: synth its RHS (recording the type) and, if a target
    /// shape constraint matches the predicate, check against it.
    pub fn check_property(&mut self, expr_id: ExprId, prop: &HirProperty) {
        // Always synth the RHS so its type is recorded in `entries` (provenance
        // / hover consume this even when there is no backward constraint).
        let actual = self.synth(expr_id, &prop.value);

        // Backward check against the resolved shape, if any.
        let predicate_iri = match &prop.key {
            PropertyKey::PrefixedName { iri } => Some(iri.clone()),
            PropertyKey::Iri => None, // the subject `iri =` is not a shape predicate
        };
        if let (Some(actual), Some(pred), Some(shape)) =
            (actual, predicate_iri, self.resolved_shape.as_ref())
            && let Some(constraint) = shape.constraint_for(pred.as_str())
        {
            let expected = constraint
                .value_ty
                .unwrap_or_else(|| Ty::new(self.db, TyKind::Iri));
            let cardinality = constraint.cardinality;
            let dest = BlamePos::ShapeProperty {
                shape: shape.shape_id,
                property: pred.clone(),
            };
            let _ = compatible(self, actual, expected, cardinality, expr_id, &dest);
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

    /// Implicit closure synthesis HOOK — implemented by plan 03-06 (Wave 4).
    ///
    /// Implicit closure synthesis is the ONLY lambda form in Fossil. This plan
    /// leaves the hook returning an `Error` type + a "not yet implemented"
    /// diagnostic; plan 03-06 fills it in. NOT mock-filled.
    pub fn synthesize_closure(
        &mut self,
        _arg: &HirExpr,
        _expected: Ty<'db>,
        _row: Ty<'db>,
    ) -> Ty<'db> {
        let eg = delay_span_bug(
            self.db,
            Span { start: 0, end: 0 },
            "implicit closure synthesis is not yet implemented in Phase 3 \
             plan 03-05 (deferred to plan 03-06)",
        );
        self.record_error(eg);
        Ty::new(self.db, TyKind::Error(eg))
    }

    /// Surface `ShEx` lowering errors (`OneOf` rejection → SC#4 split
    /// suggestion) as diagnostics on this mapping. Informational — does NOT
    /// error the mapping out (the body may still check the non-`OneOf` parts).
    // `literal_string_with_formatting_args`: the placeholder Fossil IRI
    // template (`${ex:}item/${.id}`) passed to the suggestion generator is
    // LITERAL Fossil source, not a Rust format string.
    #[allow(clippy::literal_string_with_formatting_args)]
    fn surface_shape_lowering_errors(&mut self) {
        let Some(shape) = self.resolved_shape.as_ref() else {
            return;
        };
        let db = self.db;
        let header_span = Span { start: 0, end: 0 };
        // Clone the data we need so we don't hold a borrow of `self` across the
        // mutable `record_error` calls.
        let errors = shape.errors.clone();
        let base_name = self.mapping_name();
        for err in &errors {
            match err {
                fossil_descriptors_output::ShExLoweringError::OneOfRejection(rej) => {
                    let suggestion = fossil_descriptors_output::generate_split_suggestion(
                        base_name.as_str(),
                        // Phase 3 v0.1 has no structured access to the consuming
                        // mapping's iri-template / from-clause text here; pass
                        // placeholders that the suggestion generator fills with
                        // the disjunct predicates. (The corpus test in plan
                        // 03-08 pins the exact rendered text.)
                        "`${ex:}item/${.id}`",
                        base_name.as_str(),
                        rej.shape_iri.to_string().as_str(),
                        &rej.suggestion_seed.one_of_node,
                    );
                    let msg = format!(
                        "ShEx OneOf is not supported in v0.1 ({} disjuncts in \
                         shape `{}`); help: split into {} separate mappings \
                         (one per disjunct). The split-into-mappings suggestion \
                         is provided programmatically (see \
                         `Diagnostic.suggestion_source`).",
                        rej.disjunct_count, rej.shape_iri, rej.disjunct_count,
                    );
                    // Structured suggestion carrier (Blocker #3) — NOT a
                    // Markdown delimiter. Accumulate directly (informational;
                    // no ErrorGuaranteed).
                    Diagnostic::new(Severity::Error, msg, header_span)
                        .with_suggestion_source(suggestion)
                        .accumulate(db);
                }
                fossil_descriptors_output::ShExLoweringError::CyclicShapeRef { path } => {
                    let eg = delay_span_bug(
                        db,
                        header_span,
                        format!(
                            "cyclic ShEx shape graph not supported: {}",
                            path.join(" -> ")
                        ),
                    );
                    self.record_error(eg);
                }
                fossil_descriptors_output::ShExLoweringError::UnresolvedRef { label, in_shape } => {
                    let eg = delay_span_bug(
                        db,
                        header_span,
                        format!(
                            "unresolved ShEx triple-expression ref `{label}` in shape `{in_shape}`"
                        ),
                    );
                    self.record_error(eg);
                }
                fossil_descriptors_output::ShExLoweringError::MalformedSchema(m) => {
                    let eg = delay_span_bug(db, header_span, format!("malformed ShEx schema: {m}"));
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
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "check_tests.rs"]
mod tests;
