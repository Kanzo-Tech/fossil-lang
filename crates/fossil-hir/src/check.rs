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
use crate::lower::{CmpOp, HirExpr, HirProperty, InterpolationPart, PropertyKey, lower_to_hir};
use crate::provenance::{ExprTypeEntry, ExprTypes, Provenance, ProvenanceKind};
use crate::shapes::{ResolvedShape, resolve_target_shape};
use crate::spans::{Spans, spans};
use crate::ty::display::render_ty_kind;
use fossil_graph_schema::Primitive;

use crate::ty::{ShapeId, Ty, TyKind};
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
    // A source pipeline whose row algebra does not add up taints the mapping and
    // stops here (ADR-0054 §4). Checking the body against a row that could not be
    // built would report a second, invented error for every property that reads a
    // column the join was supposed to bring.
    let source_row = resolve_source_row(db, mapping)?;
    // This used to pass `ACCEPT_ALL_DEFAULT` — one literal that turned backward
    // checking off for every program compiled through the checker, because the
    // in-query path had no descriptor to thread and the argument demanded one.
    // `resolve_target_shape` now reads the document the PROGRAM names, through
    // `System::read_file`, which registers no Salsa input dependency — so
    // `MAX_PER_MAPPING_FAN_OUT` is still `1` and the ten-mapping invalidation
    // fixture is unmoved. That was the whole reason for the argument.
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
        subject_position: false,
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
    /// Whether the expression being synthesised sits in the subject position
    /// (`iri = …`, which ADR-0057's seventh amendment respells `@subject`).
    ///
    /// It exists because an interpolation's type is not a property of the
    /// expression: `"…{u.id}"` yields an IRI under `iri =` and a string
    /// anywhere else. That is the amendment's §3 read literally — "there is no
    /// IRI template: there is interpolation" — and the alternative is a value
    /// position typed `IriTemplate`, which would fail against any shape that
    /// declares its predicate a string.
    pub(crate) subject_position: bool,
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

/// Does `e` contain at least one free `.field` reference?
///
/// CORE-07: an arg expression in a `Fn(Record<R> -> τ)` position is lifted to
/// an implicit closure ONLY when it references the row via a `.field` access
/// (otherwise it is an already-evaluated value, e.g. a literal predicate, and
/// gets the standard `check` path).
///
/// The walk is recursive over the forms that carry sub-expressions (`Call`
/// today; `Pipeline` / `Ternary` / `BinOp` as they land) — the algorithm is the
/// same at every depth: ANY descendant `FieldRef` triggers synthesis.
fn expr_contains_free_field_refs(e: &HirExpr) -> bool {
    match e {
        // Both spellings of a column reference are row-dependent. They will be
        // one spelling once `FieldRef` goes (ADR-0057, ninth amendment).
        HirExpr::FieldRef(_) | HirExpr::ColumnRef { .. } => true,
        // `clean.trim(.name)` in a closure position IS row-dependent.
        HirExpr::Call { args, .. } => args.iter().any(expr_contains_free_field_refs),
        // `.age >= 18` is the shape a filter predicate has.
        HirExpr::BinOp { lhs, rhs, .. } => {
            expr_contains_free_field_refs(lhs) || expr_contains_free_field_refs(rhs)
        }
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => {
            expr_contains_free_field_refs(cond)
                || expr_contains_free_field_refs(then)
                || expr_contains_free_field_refs(otherwise)
        }
        // An interpolation is row-dependent exactly when one of its holes is.
        // This used to read "a `Template` MAY contain `${.id}` textually, but
        // those are not yet a structured `FieldRef` HIR node — template parsing
        // is deferred". The tree landed; the hole is an expression, and it is
        // walked like every other.
        HirExpr::Interpolation(parts) => parts.iter().any(|p| match p {
            InterpolationPart::Text(_) => false,
            InterpolationPart::Hole(e) => expr_contains_free_field_refs(e),
        }),
        // Leaf forms with no `.field` descendants.
        HirExpr::StringLit(_) | HirExpr::IntLit(_) | HirExpr::PrefixedName { .. } => false,
    }
}

/// Textual rewrite for closure-binding DISPLAY only: every standalone
/// `.identifier` becomes `row.identifier`. Used to render the implicit closure
/// parameter binding (e.g. `.age >= 18` → `row.age >= 18`); NOT used for
/// type-checking (the checker walks the [`HirExpr`] structure directly).
///
/// Heuristic state machine (no regex): a `.` begins a field reference when it
/// is followed by an ASCII-alphabetic identifier-start char AND it is NOT
/// itself preceded by an identifier char or another `.` (which would make it a
/// member-access chain / decimal point rather than a bare field ref). Phase 3
/// v0.1's surface `.field` grammar is single-segment (no `.a.b` chaining), so a
/// single `row` insertion per bare `.ident` is correct.
fn rewrite_field_refs_to_row_dot(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut prev: Option<char> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == '.' {
            let next_is_ident_start = chars
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '_');
            let prev_blocks =
                matches!(prev, Some(p) if p.is_ascii_alphanumeric() || p == '_' || p == '.');
            if next_is_ident_start && !prev_blocks {
                out.push_str("row");
            }
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Structural rendering of a leaf [`HirExpr`] as closure-body text, used when
/// no real source span is available (the test-driven path — there is no surface
/// pipeline/call syntax in Phase 3 v0.1, so closure synthesis is validated by
/// directly constructed `HirExpr`s without arena spans). A `FieldRef("age")`
/// renders as `.age` (which `rewrite_field_refs_to_row_dot` then turns into
/// `row.age`); literals render verbatim.
fn render_leaf_expr_text(e: &HirExpr) -> String {
    match e {
        HirExpr::FieldRef(name) => format!(".{name}"),
        HirExpr::ColumnRef { binding, column } => format!("{binding}.{column}"),
        HirExpr::StringLit(s) => format!("\"{s}\""),
        // Rendered in the spelling that survives, not the one it was written
        // in: the backtick and `${` are on their way out.
        HirExpr::Interpolation(parts) => {
            let body: String = parts
                .iter()
                .map(|p| match p {
                    InterpolationPart::Text(t) => t.replace('{', "{{"),
                    InterpolationPart::Hole(e) => format!("{{{}}}", render_leaf_expr_text(e)),
                })
                .collect();
            format!("\"{body}\"")
        }
        HirExpr::PrefixedName { iri } => iri.to_string(),
        HirExpr::Call { func, args } => {
            let rendered: Vec<String> = args.iter().map(render_leaf_expr_text).collect();
            format!("{func}({})", rendered.join(", "))
        }
        HirExpr::IntLit(v) => v.to_string(),
        HirExpr::BinOp { op, lhs, rhs } => format!(
            "{} {} {}",
            render_leaf_expr_text(lhs),
            op_text(*op),
            render_leaf_expr_text(rhs)
        ),
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => format!(
            "{} ? {} : {}",
            render_leaf_expr_text(cond),
            render_leaf_expr_text(then),
            render_leaf_expr_text(otherwise)
        ),
    }
}

/// The source spelling of an operator, for diagnostics and closure display.
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
    }
}

/// Render an implicit-closure binding for diagnostic + hover display (CORE-07,
/// SC#3). Produces text like
/// `(row: Record<{id: String, age: Integer}>) => row.age >= 18`.
///
/// CRITICAL (Risk Register): the `row` Record type is rendered via
/// [`render_ty_kind`] (the shared `TyDisplay`) — never raw `{:?}` Debug. The
/// returned string MUST NOT contain `Unknown` or `InferenceId`; any internal
/// inference-state placeholder normalises to `?` at the display boundary inside
/// `render_ty_kind`.
fn render_closure<'db>(
    db: &'db dyn fossil_base::Db,
    row_ty: Ty<'db>,
    expr_source_text: &str,
) -> SmolStr {
    let row_display = render_record_for_closure(db, row_ty);
    let body_text = rewrite_field_refs_to_row_dot(expr_source_text.trim());
    SmolStr::from(format!("(row: {row_display}) => {body_text}"))
}

/// Render the closure's `row` parameter type. For a `Record`, expand the field
/// list (`Record<{id: String, age: Integer}>`) — the bare `render_ty_kind`
/// `Record { ... }` form would hide the field names the hover needs to surface.
/// Falls back to `render_ty_kind` for any non-Record row (defensive).
fn render_record_for_closure<'db>(db: &'db dyn fossil_base::Db, row_ty: Ty<'db>) -> String {
    match row_ty.kind(db) {
        TyKind::Record(rec) => {
            let fields: Vec<String> = rec
                .fields(db)
                .iter()
                .map(|f| format!("{}: {}", f.name, render_ty_kind(db, f.ty.kind(db))))
                .collect();
            format!("Record<{{{}}}>", fields.join(", "))
        }
        other => render_ty_kind(db, other),
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
                let kind = if self.subject_position {
                    TyKind::IriTemplate
                } else {
                    TyKind::Primitive(Primitive::String)
                };
                (Ty::new(db, kind), ProvenanceKind::Literal)
            }
            // T-PrefixedName: an IRI literal.
            HirExpr::PrefixedName { .. } => (Ty::new(db, TyKind::Iri), ProvenanceKind::Literal),
            // T-Column: the qualified spelling. Same resolution as T-Field,
            // plus the check the anonymous form could never make — that the
            // name on the left is the row this mapping actually reads. That
            // check is the point of qualifying (ADR-0057, ninth amendment).
            //
            // It is an equality only because one row is in scope today. §1 of
            // that amendment puts two there — `join` leaves both named — so
            // this becomes a lookup over the scope when step 4 lands.
            HirExpr::ColumnRef { binding, column } => {
                let source_name = self.source_binding_name();
                if binding != &source_name {
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
                let ty = self.lookup_field(expr_id, column)?;
                (
                    ty,
                    ProvenanceKind::InputDescriptor {
                        source_name,
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
            HirExpr::IntLit(_) => (
                Ty::new(db, TyKind::Primitive(Primitive::Integer)),
                ProvenanceKind::Literal,
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

    /// Checking-mode entry. Phase 3 v0.1: synth then [`compatible`].
    ///
    /// CORE-07 fast-path: when `expected` is a single-param `Fn(Record<R> -> τ)`
    /// AND the arg `e` contains free `.field` references, lift `e` to an
    /// implicit closure (the ONLY lambda form in Fossil — type-system.md §7)
    /// via [`Self::synthesize_closure`]. The synthesised closure has the `Fn`
    /// type itself (which trivially satisfies `expected` by S-Refl), so we
    /// return `Ok(())` once synthesis succeeds.
    pub fn check(
        &mut self,
        expr_id: ExprId,
        e: &HirExpr,
        expected: Ty<'db>,
        cardinality: Cardinality,
        dest: &BlamePos,
    ) -> Result<(), ErrorGuaranteed> {
        // CORE-07 implicit closure synthesis fast-path.
        if let TyKind::Fn(sig) = expected.kind(self.db) {
            // Single-parameter only — multi-arg lambdas are out of scope
            // (PROJECT.md "Out of Scope").
            let params = sig.params(self.db);
            if params.len() == 1 {
                let param_ty = params[0];
                if matches!(param_ty.kind(self.db), TyKind::Record(_))
                    && expr_contains_free_field_refs(e)
                {
                    let closure_ty = self.synthesize_closure(expr_id, e, expected, param_ty);
                    // synthesize_closure type-checks the body in the row context
                    // and records the SynthesizedClosureRendering provenance. The
                    // closure value has the `Fn` type — S-Refl against `expected`.
                    return compatible(self, closure_ty, expected, cardinality, expr_id, dest);
                }
                // No free FieldRefs → not a row-dependent predicate; fall
                // through to the standard `synth` + `compatible` path (the arg
                // is an already-typed value, e.g. a named Fn).
            }
        }
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
        self.subject_position = matches!(prop.key, PropertyKey::Iri);
        let actual = self.synth(expr_id, &prop.value);
        self.subject_position = false;

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
        let Some(entry) = crate::stdlib::stdlib().lookup(func.as_str()) else {
            let names = crate::stdlib::stdlib().iter().map(|e| e.name.as_str());
            let msg = crate::didyoumean::did_you_mean(func.as_str(), names).map_or_else(
                || format!("unknown function `{func}`"),
                |s| format!("unknown function `{func}` — did you mean `{s}`?"),
            );
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
            let expected = param.to_ty(db);
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
                let eg = delay_span_bug(
                    db,
                    self.span_of(expr_id),
                    format!(
                        "argument {} of `{func}` expects {}, but this is {}",
                        i + 1,
                        render_ty_kind(db, expected.kind(db)),
                        render_ty_kind(db, actual.kind(db)),
                    ),
                );
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
    /// makes it always present. The result is `Bool` regardless, because the
    /// operator says so and the operands cannot change that.
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
        }
        bool_ty
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
                // into Float, and a value widens into its Optional).
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

    /// Implicit closure synthesis (CORE-07, type-system.md §7 T-Closure) —
    /// plan 03-06 fills the plan-03-05 hook.
    ///
    /// Implicit closure synthesis is the ONLY lambda form in Fossil: there is no
    /// surface `\row -> ...` syntax. Given an arg expression that contains free
    /// `.field` references and an expected type `Fn(Record<R> -> τ_pred)`, bind
    /// `row: Record<R>` and type-check the original expression `arg` against
    /// `τ_pred` in that row context. The closure is METADATA wrapped around the
    /// existing [`HirExpr`] — NOT a new HIR node (RESEARCH.md §"Don't
    /// Hand-Roll"), so the lowering arena is unchanged.
    ///
    /// Records a [`ProvenanceKind::SynthesizedClosureRendering`] entry on
    /// `expr_id` (the closure body's outer id) so plan 03-07's LSP hover (SC#3)
    /// can surface the binding. The synthesis is NEVER hidden from the user.
    ///
    /// Returns `expected` (the `Fn` type) on success — the closure VALUE has the
    /// function type, not its body type — or a [`TyKind::Error`] type if the
    /// precondition (an `Fn` expected type) is violated. Type errors INSIDE the
    /// body (missing field, type mismatch) emit standard diagnostics via the
    /// recursive [`Self::check`] call and do NOT swallow the closure rendering.
    pub fn synthesize_closure(
        &mut self,
        expr_id: ExprId,
        arg: &HirExpr,
        expected: Ty<'db>,
        row: Ty<'db>,
    ) -> Ty<'db> {
        let db = self.db;
        // Precondition (asserted by `check`): `expected` is `Fn(sig)`.
        let TyKind::Fn(sig) = expected.kind(db) else {
            let eg = delay_span_bug(
                db,
                self.span_of(expr_id),
                "internal: synthesize_closure called with a non-Fn expected type",
            );
            self.record_error(eg);
            return Ty::new(db, TyKind::Error(eg));
        };
        let result_ty = sig.return_ty(db);

        // Swap the row context: inside the closure body, `.field` resolves
        // against `row` (the closure parameter's Record), not the outer source
        // row. Restore afterwards regardless of the check outcome.
        let prior_source_row = self.source_row;
        self.source_row = Some(row);

        // Type-check the body against the predicate's result type IN the row
        // context. We call `synth` + `compatible` directly (NOT `self.check`)
        // to avoid re-entering the Fn-typed fast-path on `result_ty` (which is
        // a value type, e.g. Bool, not an Fn — so it would not recurse anyway,
        // but going through synth keeps the entry recording explicit).
        if let Some(actual) = self.synth(expr_id, arg) {
            let _ = compatible(
                self,
                actual,
                result_ty,
                Cardinality::Exact(1),
                expr_id,
                &BlamePos::Expr(expr_id),
            );
        }
        // (If synth returned None — e.g. a missing field — it already emitted a
        // did-you-mean diagnostic + recorded the error. The closure rendering is
        // still produced below against the row's ACTUAL fields.)

        self.source_row = prior_source_row;

        // Build the rendering from a STRUCTURAL rendering of the leaf
        // `HirExpr`. Phase 3 v0.1 has no surface pipeline/call syntax, so a
        // closure body is always one of the four leaf forms — rendering it
        // structurally (`.age` → `row.age`, literals verbatim) is exact and
        // span-independent. When a later phase adds the Pratt-lowered
        // expression tree (and real arena spans for closure-arg positions), the
        // body can be sliced from the original source verbatim (`file.text(db)`
        // by span) for richer multi-token bodies; not needed in v0.1 because no
        // surface form produces a closure-arg span yet.
        let span = self.span_of(expr_id);
        let body_for_render = render_leaf_expr_text(arg);
        let rendering = render_closure(db, row, &body_for_render);

        // Amend the entry `synth` pushed for `expr_id` (replace its provenance
        // with the closure rendering), or push a fresh one if `synth` recorded
        // none (e.g. the field-not-found path returned None).
        let provenance = Provenance {
            span,
            kind: ProvenanceKind::SynthesizedClosureRendering { rendering },
        };
        if let Some(entry) = self.entries.iter_mut().rev().find(|e| e.expr_id == expr_id) {
            entry.provenance = provenance;
        } else {
            self.entries.push(ExprTypeEntry {
                expr_id,
                ty: result_ty,
                provenance,
            });
        }

        // The closure value has the `Fn` type — return `expected` (S-Refl).
        expected
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
