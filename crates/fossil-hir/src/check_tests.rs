//! Integration tests for the bidirectional checker.
//!
//! Covers: literal-subset regression under widening, forward propagation into
//! the source row + did-you-mean, backward cardinality blame against a target
//! shape, the 5 subtyping rules, value-disjunction rejection, the `expr_types`
//! thin-accessor inversion, and the `ErrorGuaranteed`-implies-diagnostic
//! contract.

use super::*;
use crate::body::ExprId;
use crate::def_map::{MappingLoc, def_map};
use crate::lower::{HirExpr, HirProperty, PropertyKey};
use crate::provenance::{expr_types, ty_origin};
use crate::shapes::ResolvedShape;
use crate::ty::{Ty, TyKind};
use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::{Occurs, Primitive};
use std::sync::Arc;

const HELLO: &str = "\
type { Person } := io.shex(\"personas.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = users.name
";

fn db_with(src: &str) -> (FossilDb, SourceFile) {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, src.to_string(), "test.fossil".to_string());
    (db, file)
}

fn first_mapping(db: &FossilDb, file: SourceFile) -> MappingLoc<'_> {
    *def_map(db, file)
        .mappings(db)
        .first()
        .expect("at least one mapping")
}

/// The `users` row every checker test below builds on: `id`/`age` integers, a
/// `name` string.
///
/// It is an INFERRED descriptor — what a host registers after introspecting the
/// file — built here by hand and passed straight to `record_from_inferred`.
fn users_row(db: &dyn fossil_base::Db) -> Ty<'_> {
    crate::infer::record_from_inferred(
        db,
        &InferredDescriptor {
            uri: "users.csv".into(),
            columns: vec![
                InferredColumn {
                    name: "id".into(),
                    primitive: Primitive::Integer,
                },
                InferredColumn {
                    name: "name".into(),
                    primitive: Primitive::String,
                },
                InferredColumn {
                    name: "age".into(),
                    primitive: Primitive::Integer,
                },
            ],
            freshness_token: String::new(),
        },
    )
}

/// Build a `Checker` over a fixture, with an explicit source row + optional
/// resolved shape, inside a tracked shim so `delay_span_bug` is valid.
fn build_checker<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    source_row: Option<Ty<'db>>,
    resolved_shape: Option<ResolvedShape<'db>>,
) -> Checker<'db> {
    // The short-name table `typecheck_mapping` builds for the real path. A
    // bare key resolves against it, so a `Checker` built without one would
    // reject every property in these tests as a name the shape does not
    // declare.
    // No `@rename` in any fixture here: every short name is the last segment of
    // its predicate IRI.
    let predicates = resolved_shape
        .as_ref()
        .map_or_else(Vec::new, |s| s.short_names(&[]).0);
    // The scope the `from` clause really puts in the body — resolved by the
    // production path, so `from Adults` presents `User` and not `Adults`. It is
    // NOT derived from `source_row`: the row-name check has to fire with no
    // schema at all, which is what `a_column_ref_naming_a_foreign_row_is_rejected`
    // pins.
    //
    // When a test supplies an explicit row it stands in for the ONE binding
    // these fixtures draw on; a fixture with a join would need its rows named,
    // and there is none here.
    let resolved = crate::infer::resolve_source_scope(db, mapping)
        .ok()
        .flatten();
    let source_scope = match (resolved, source_row) {
        (Some(scope), Some(_)) => scope
            .bindings()
            .next()
            .map(|b| crate::ty::Rows::one(b, source_row)),
        (scope, _) => scope,
    };
    Checker {
        expr: crate::check::Expr {
            db,
            file: mapping.file(db),
            flat: source_row,
            rows: source_scope,
            relation: crate::check::source_binding_name(db, mapping),
            spans: crate::check::SpanSource::Table(spans(db, mapping)),
            entries: Vec::new(),
            first_error: None,
            expected_ref: None,
        },
        mapping,
        resolved_shape,
        predicates,
        // No `@rename` in any fixture here, matching the `short_names(&[])`
        // above: every short name is the last segment of its predicate IRI.
        renames: Vec::new(),
    }
}

/// The diagnostics a shim raised about the CODE under test.
///
/// `build_checker` resolves the real source scope now — `from Adults` has to
/// present `User` — and that reads `lower_to_hir`, so every shim's accumulator
/// subtree carries the FIXTURE's own complaints as well: these fixtures name
/// `io.shex("personas.shex")` and the bare `NativeSystem` they run on installs
/// no row that reads types, so the shape name binds nothing and `lower_to_hir`
/// says so, once per shim. Counting those is a hostage to what the fixture
/// declares; this filters them out by the one thing they all say.
fn about_the_body(diags: &[&Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| d.message.clone())
        .filter(|m| !m.contains("is declared and bound nothing"))
        .collect()
}

// ── Literal-subset regression (Phase 2 behaviour preserved) ────────────────

#[test]
fn literal_subset_regression() {
    // Property 0 of hello is `@subject = "…{…}…"`. It used to synthesise
    // `IriTemplate` — «a string with holes» — and what it means is the identity
    // of the node this mapping produces, so it is a REFERENCE to the shape the
    // mapping targets. The interpolation is unchanged; the type says what it
    // is for.
    let (db, file) = db_with(HELLO);
    let m = first_mapping(&db, file);
    let entry = ty_origin(&db, m, ExprId(0)).expect("property 0 (the identity) must have a type");
    // `personas.shex` is not registered in this fixture, so `Person` binds
    // nothing — and an identity whose shape did not resolve is a reference to
    // the empty set, the same answer an edge to an unresolvable target gets.
    assert_eq!(entry.ty, Ty::reference(&db, std::iter::empty()));
}

#[test]
fn fieldref_without_schema_synthesises_no_type_phase_2_compat() {
    // hello's `users` source has NO descriptor registered → source_row = None →
    // the `users.name` reference synthesises no entry (preserving Phase 2's None, keeping
    // the walking-skeleton free of spurious errors).
    let (db, file) = db_with(HELLO);
    let m = first_mapping(&db, file);
    assert!(
        ty_origin(&db, m, ExprId(1)).is_none(),
        "FieldRef with no source schema must not synthesise a type"
    );
    // And the whole mapping type-checks OK (no shape, no schema).
    assert!(typecheck_mapping(&db, m).is_ok());
}

// ── Forward propagation (SC#1) ─────────────────────────────────────────────

#[test]
fn fieldref_propagates_string_from_inferred_descriptor() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let ty = cx.expr.lookup_field(ExprId(0), "name")?;
        Some(render_ty_kind(db, ty.kind(db)))
    }

    let (db, file) = db_with(HELLO);
    let rendered = shim(&db, file).expect("name lookup");
    assert_eq!(
        rendered, "String",
        "`name` resolves to String via the inferred row"
    );
}

#[test]
fn fieldref_typo_emits_did_you_mean_against_the_inferred_row() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<()> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let _ = cx.expr.lookup_field(ExprId(0), "naem"); // typo for `name`
        Some(())
    }

    let (db, file) = db_with(HELLO);
    let _ = shim(&db, file);
    let raised = shim::accumulated::<Diagnostic>(&db, file);
    let blame: Vec<&&Diagnostic> = raised
        .iter()
        .filter(|d| !d.message.contains("is declared and bound nothing"))
        .collect();
    assert_eq!(
        blame.len(),
        1,
        "exactly one field-not-found diagnostic, got {blame:#?}"
    );
    let d = blame[0];
    // The relation is named even though the SPELLING did not name it: `naem` is
    // a bare reference resolved against the flat row, and `refuse_column` reads
    // the relation the rows came from. One sentence for both spellings — the
    // qualified path said `unknown column `x` on `Y`` and this one said
    // `unknown column `x``, which is two statements of one fact.
    assert_eq!(
        d.message, "`naem` is not a field of `users`",
        "the refusal names the column and the relation"
    );
    // The repair is a FIELD. It used to be a clause of the message, and the two
    // readers that wanted it (`fossil-cli`, the conformance harness) each
    // searched the message text for `did you mean` to get it back out.
    assert_eq!(
        d.help.as_deref(),
        Some("did you mean `name`?"),
        "the near miss is the `help:`, not part of the sentence"
    );
    assert!(
        !d.message.contains("did you mean"),
        "and it is not in both places: {:?}",
        d.message
    );
}

/// The caret is the NAME, and the quick-fix replaces exactly it.
///
/// The fixture WRITES the typo, and it has to: the table is read off the CST
/// and keyed by name, so a lookup for a name the source does not contain finds
/// nothing and falls back to the right-hand side. That fallback is the designed
/// behaviour and it is what the first draft of this test measured by accident.
///
/// Sliced out of the source rather than compared to two numbers — the numbers
/// are what is under test, and `86..96` versus `92..96` is not a difference
/// anyone reads.
///
/// **Both halves were unreachable before `ref_spans`.** The finest range the
/// compiler recorded was a whole right-hand side, so the caret covered
/// `users.name` and `Diagnostic::did_you_mean` — the structured
/// `(wrong_span, replacement)` pair `fossil_ide::code_action` builds a
/// `WorkspaceEdit` from — was never populated by anything but a test. A
/// quick-fix over the wider span would have deleted `users.` along with the
/// typo, which is why `refuse_column` offers it only when the span IS the name.
#[test]
fn the_caret_and_the_quick_fix_cover_the_name_and_nothing_else() {
    const TYPO: &str = "\
type { Person } := io.shex(\"personas.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"https://example.org/u/{users.id}\"
    name = users.naem
";
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<()> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        // Property 1 is `name = users.naem`, so `ExprId(1)` is the right-hand
        // side this reference is written in.
        let _ = cx.expr.lookup_column(ExprId(1), "users", "naem");
        Some(())
    }

    let (db, file) = db_with(TYPO);
    let _ = shim(&db, file);
    let raised = shim::accumulated::<Diagnostic>(&db, file);
    let d = raised
        .iter()
        .find(|d| d.message.contains("is not a field of"))
        .expect("the column is refused");

    // Mapping-relative, so rebase before slicing: `HELLO`'s mapping starts at
    // the `User :` line.
    let base = crate::spans::mapping_start_offset(&db, first_mapping(&db, file));
    let slice = |s: fossil_base::Span| {
        let start = (base + s.start) as usize;
        let end = (base + s.end) as usize;
        &TYPO[start..end]
    };

    assert_eq!(
        slice(d.span),
        "naem",
        "the caret is on the column, not on `users.naem`"
    );
    let dym = d
        .did_you_mean
        .as_ref()
        .expect("a near miss carries a quick-fix");
    assert_eq!(
        slice(dym.wrong_span),
        "naem",
        "and the edit replaces exactly the typo — a wider span deletes `users.`"
    );
    assert_eq!(dym.replacement, "name");
}

/// One property writing two rows blames the RIGHT one.
///
/// `ref_span` matches the BINDING as well as the name, and this is why: after a
/// join, `"{Purchase.id}-{User.id}"` writes `id` twice, and a table keyed by
/// name alone answers with the first. Both are spelled `id`, both are real
/// columns of their own row, and only one of them is the reference being
/// refused — so the caret would land on the reference that is CORRECT and the
/// quick-fix would rewrite it.
///
/// The `.expect` is the assertion that matters: the fallback to the whole
/// right-hand side is silent by design, so a lookup that finds the wrong entry
/// and a lookup that finds none are both invisible from the outside.
#[test]
fn two_rows_writing_one_column_name_blame_the_one_that_is_wrong() {
    const JOINED: &str = "\
type { Person } := io.shex(\"personas.shex\")
Purchase := io.csv(\"o.csv\")
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"https://example.org/u/{Purchase.id}-{users.id}\"
    name = users.name
";
    let (db, file) = db_with(JOINED);
    let m = first_mapping(&db, file);
    let table = spans(&db, m);
    let base = crate::spans::mapping_start_offset(&db, m);
    let slice = |s: fossil_base::Span| {
        let start = (base + s.start) as usize;
        &JOINED[start..(base + s.end) as usize]
    };

    // Property 0 is the identity, and both `id`s are written inside it.
    let purchase = table
        .ref_span(&db, ExprId(0), Some("Purchase"), "id")
        .expect("`Purchase.id` is written in the identity");
    let user = table
        .ref_span(&db, ExprId(0), Some("users"), "id")
        .expect("and so is `users.id`");
    assert_ne!(
        purchase, user,
        "two references spelled `id` are two spans, not one"
    );
    assert_eq!(slice(purchase), "id");
    assert_eq!(slice(user), "id");
    assert!(
        purchase.start < user.start,
        "and they are in source order: {purchase:?} then {user:?}"
    );
}

// ── Qualified column references ────────────────────────────────────────────

/// A relation derived from `User`, so that the two tests below can ask the
/// question the equality could not: `User.name` is legal from `from Adults`,
/// and `Orders.name` is not.
const DERIVED: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Orders := io.csv(\"o.csv\")
Adults := User.where(User.age >= 18)
Users : Person from Adults
    @subject = \"https://example.org/u/{User.id}\"
    name = User.name
";

/// The foreign-row refusals in `diags`, by their text.
///
/// It was `diags.len() == 1`, and that counted every diagnostic the FIXTURE
/// raises as well — `personas.shex` is not readable by the bare `NativeSystem`
/// these tests use, so the shape name does not resolve and `lower_to_hir` says
/// so. Counting the refusal under test is the assertion; counting the fixture's
/// own noise is a hostage to whatever the fixture declares next.
fn foreign_row_refusals(diags: &[&Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| d.message.clone())
        .filter(|m| m.contains("reads a row this mapping does not have"))
        .collect()
}

#[test]
fn a_column_ref_naming_a_foreign_row_is_rejected() {
    // The check the anonymous `.name` could never make: `Orders.user_id` in a
    // mapping that reads `User` names a row this mapping does not have. It
    // fires before the column is resolved, so it does not need a schema.
    //
    // **`Orders` is a binding this file declares**, and that is the point: the
    // refusal is not about a name nobody bound, it is about a row this RELATION
    // does not carry. That is the case the scope must keep rejecting.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let mut cx = build_checker(db, m, None, None);
        let prop = HirProperty {
            key: PropertyKey::Name(smol_str::SmolStr::from("x")),
            value: HirExpr::ColumnRef {
                binding: smol_str::SmolStr::from("Orders"),
                column: smol_str::SmolStr::from("user_id"),
            },
        };
        cx.check_property(ExprId(0), &prop);
        cx.expr.first_error.is_some()
    }

    let (db, file) = db_with(DERIVED);
    assert!(shim(&db, file), "a foreign row must record an error");
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    let refusals = foreign_row_refusals(&diags);
    assert_eq!(
        refusals.len(),
        1,
        "exactly one foreign-row diagnostic, got {refusals:?}"
    );
    let msg = &refusals[0];
    assert!(
        msg.contains("Orders.user_id") && msg.contains("Adults"),
        "the diagnostic must name the row asked for AND the relation mapped, got {msg:?}"
    );
}

#[test]
fn a_column_ref_naming_a_row_the_relation_derives_from_is_accepted() {
    // **The rule `grammar.bnf` states under `SourceDef`**: *«a mapping body
    // writes `User.name` and never `Adults.name`, even when it draws
    // `from Adults`»*. `Users` draws `from Adults` and writes `User.name`, and
    // this used to be refused by an equality against the `from` name — the
    // defect that failed nine of the twenty-three conformance programs.
    //
    // `DERIVED` registers no descriptor, so the column synthesises no type,
    // exactly as `fieldref_without_schema_synthesises_no_type_phase_2_compat`
    // describes. What is asserted is that qualifying it raises no error of its
    // own.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return true;
        };
        let mut cx = build_checker(db, m, None, None);
        let prop = HirProperty {
            key: PropertyKey::Name(smol_str::SmolStr::from("x")),
            value: HirExpr::ColumnRef {
                binding: smol_str::SmolStr::from("User"),
                column: smol_str::SmolStr::from("name"),
            },
        };
        cx.check_property(ExprId(0), &prop);
        cx.expr.first_error.is_some()
    }

    let (db, file) = db_with(DERIVED);
    assert!(
        !shim(&db, file),
        "the binding the relation derives from must not error"
    );
}

#[test]
fn a_column_ref_naming_the_relations_own_name_is_rejected() {
    // The other half of the same rule, and the half that stops the fix from
    // being «accept anything»: `Adults` is the RELATION, not a row, so
    // `Adults.name` is refused with the same words a stranger's name gets.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let mut cx = build_checker(db, m, None, None);
        let prop = HirProperty {
            key: PropertyKey::Name(smol_str::SmolStr::from("x")),
            value: HirExpr::ColumnRef {
                binding: smol_str::SmolStr::from("Adults"),
                column: smol_str::SmolStr::from("name"),
            },
        };
        cx.check_property(ExprId(0), &prop);
        cx.expr.first_error.is_some()
    }

    let (db, file) = db_with(DERIVED);
    assert!(shim(&db, file), "the relation's own name is not a row");
    let refusals = foreign_row_refusals(&shim::accumulated::<Diagnostic>(&db, file));
    assert_eq!(refusals.len(), 1, "exactly one refusal, got {refusals:?}");
}

// ── Subtyping rules (type-system.md §9) ────────────────────────────────────

#[test]
fn compatible_integer_widens_to_float() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let int = Ty::new(db, TyKind::Primitive(Primitive::Integer));
        let flt = Ty::new(db, TyKind::Primitive(Primitive::Float));
        let mut cx = build_checker(db, m, None, None);
        compatible(
            &mut cx.expr,
            int,
            Some(flt),
            ExprId(0),
            &HirExpr::IntLit(1),
            "age",
        )
        .is_ok()
    }

    let (db, file) = db_with(HELLO);
    assert!(shim(&db, file), "Integer <: Float (S-IntFlt) must hold");
}

#[test]
fn compatible_string_to_integer_fails() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let s = Ty::new(db, TyKind::Primitive(Primitive::String));
        let i = Ty::new(db, TyKind::Primitive(Primitive::Integer));
        let mut cx = build_checker(db, m, None, None);
        let value = HirExpr::ColumnRef {
            binding: smol_str::SmolStr::from("users"),
            column: smol_str::SmolStr::from("name"),
        };
        compatible(&mut cx.expr, s, Some(i), ExprId(0), &value, "age").is_err()
    }

    let (db, file) = db_with(HELLO);
    assert!(shim(&db, file), "String is not a subtype of Integer");
    let diags = about_the_body(&shim::accumulated::<Diagnostic>(&db, file));
    assert_eq!(diags.len(), 1, "got {diags:?}");
    let msg = &diags[0];
    assert!(
        msg.contains("String") && msg.contains("Integer"),
        "got {msg:?}"
    );
}

// Three tests lived here and all three built a `TyKind::Optional`:
// `compatible_string_lifts_into_optional` (S-Opt),
// `compatible_optional_fails_against_cardinality_one_required` and
// `check_property_against_a_target_shape_optional_fails` (the cardinality
// blame). The variant is gone because nothing but a test ever constructed one —
// so what they proved was that the checker handles a value the compiler cannot
// produce. The count a shape declares is still enforced, in the one direction
// that is observable: `check_required_properties`, covered by
// `a_named_document_that_cannot_answer_says_which_way_it_failed`'s siblings and
// by the corpus.

/// The defect, at the place it bit. A document that mentions `ex:name` and does
/// not narrow its value used to reach here as `TyKind::Iri` — the narrowest
/// type in the lattice — via `unwrap_or_else`, so the most ordinary property in
/// the corpus, `name = .name` against `ex:name .`, was a type error.
///
/// This had a second shim, `still_counts`: the same un-narrowed constraint
/// demanding one-or-more, against an `Optional<String>`, proving the `None` did
/// not turn the constraint off wholesale. It built the only kind of value that
/// could fail that check, and `TyKind::Optional` is gone because a test was the
/// only thing that ever built one.
#[test]
fn an_un_narrowed_constraint_accepts_a_string() {
    const SRC: &str = "\
type { Person } := io.shex(\"personas.shex\")
users := io.csv(\"x.csv\")
Contact : Person from users
    @subject = \"https://example.org/c/{users.id}\"
    name = users.name
";
    /// `value_ty: None`, `occurs` = exactly one. A `String` satisfies it.
    #[salsa::tracked]
    fn accepts(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let mut cx = build_checker(
            db,
            m,
            Some(row_record(db, &[("name", Primitive::String)])),
            Some(ResolvedShape {
                constraints: vec![crate::shapes::ShapeConstraint {
                    predicate: smol_str::SmolStr::from("https://example.org/name"),
                    value_ty: None,
                    occurs: Occurs::ONE,
                }],
                rejections: Vec::new(),
            }),
        );
        cx.check_property(
            ExprId(0),
            &HirProperty {
                key: PropertyKey::Name(smol_str::SmolStr::from("name")),
                value: HirExpr::FieldRef(smol_str::SmolStr::from("name")),
            },
        );
        cx.expr.first_error.is_none()
    }

    let (db, file) = db_with(SRC);
    assert!(
        accepts(&db, file),
        "a constraint that narrows nothing must accept a String; it demanded \
         `Iri` until now, so every un-narrowed predicate in the corpus was a \
         type error: {:#?}",
        accepts::accumulated::<Diagnostic>(&db, file)
    );
    let noise = about_the_body(&accepts::accumulated::<Diagnostic>(&db, file));
    assert!(
        noise.is_empty(),
        "and it must do so silently, got {noise:?}"
    );
}

/// A refused value names the SLOT in the message and underlines ITSELF in a
/// label — and nothing anywhere prints a `Span` at the author.
///
/// It went through `check_property` rather than calling `compatible` directly,
/// because half of what is asserted is that the name survives the trip: the
/// key the body wrote reaches the message, and the lowered right-hand side
/// reaches the label.
///
/// # What it was, and why the old shape passed every test there was
///
/// `expected `Float`, got `String` (expected because of the constraint at Span
/// { start: 102, end: 120 })` — with `labels` empty, so miette drew the generic
/// `here` under the value. The debug-printed span was the SOURCE's, because
/// `BlamePos::ShapeProperty` carried no location and the emitter fell back;
/// `compatible`'s own docblock called this «the two-span blame pattern». Both
/// halves were invisible to the suite: the two tests that reached `compatible`
/// asserted `msg.contains("String")`, which is true of either spelling.
#[test]
fn a_refused_value_names_the_slot_and_underlines_itself() {
    const SRC: &str = "\
type { Order } := io.shex(\"shape.shex\")
Purchase := io.csv(\"orders.csv\")
Orders : Order from Purchase
    @subject = \"https://shop.example/order/{Purchase.id}\"
    total = Purchase.reference
";
    #[salsa::tracked]
    fn refuse(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let mut cx = build_checker(
            db,
            m,
            Some(row_record(db, &[("reference", Primitive::String)])),
            Some(ResolvedShape {
                constraints: vec![crate::shapes::ShapeConstraint {
                    predicate: smol_str::SmolStr::from("https://shop.example/voc#total"),
                    value_ty: Some(Ty::new(db, TyKind::Primitive(Primitive::Float))),
                    occurs: Occurs::ONE,
                }],
                rejections: Vec::new(),
            }),
        );
        cx.check_property(
            ExprId(0),
            &HirProperty {
                key: PropertyKey::Name(smol_str::SmolStr::from("total")),
                value: HirExpr::ColumnRef {
                    binding: smol_str::SmolStr::from("Purchase"),
                    column: smol_str::SmolStr::from("reference"),
                },
            },
        );
        cx.expr.first_error.is_some()
    }

    let (db, file) = db_with(SRC);
    assert!(refuse(&db, file), "String does not satisfy Float");

    let raised = refuse::accumulated::<Diagnostic>(&db, file);
    let blame: Vec<&&Diagnostic> = raised
        .iter()
        .filter(|d| !d.message.contains("is declared and bound nothing"))
        .collect();
    assert_eq!(blame.len(), 1, "one refusal, got {blame:#?}");
    let d = blame[0];

    assert_eq!(
        d.message, "`total` expects Float, and this is String",
        "the message names the slot that refused the value"
    );
    let texts: Vec<&str> = d.labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        ["`Purchase.reference` is String"],
        "exactly one label, and it reads the value back with its type"
    );
    assert_eq!(
        d.labels[0].span, d.span,
        "the label underlines the value the diagnostic points at"
    );
    // The defect this replaces, stated as the thing that must not come back:
    // a `Span` reaching an author through `{:?}`. Checked over the message AND
    // the labels because either could carry one.
    for text in std::iter::once(d.message.as_str()).chain(texts) {
        assert!(
            !text.contains("Span {"),
            "a debug-printed span reached the author: {text:?}"
        );
    }
}

// ── The four ways a named document fails to produce a shape ────────────────

/// Every one of these was a silent `None` — the same answer as "this program
/// names no document" — so the commonest mistake, a misspelt shape name,
/// produced no message at all.
///
/// # The message moved, and the assertions moved with it
///
/// It asserted four `TargetShapeError` renderings, emitted per MAPPING by
/// `surface_target_shape_error`. A header names a bare name now, so the
/// document is read where the NAME is bound (`type { T } := io.shex(…)`,
/// `def_map`'s positional binding) and every one of these four failures lands
/// there first; by the time a mapping asks for its target shape there is no
/// shape IRI left to fail with, and `resolve_target_shape` answers `Ok(None)`.
/// `crate::shapes`'s
/// `a_document_that_cannot_answer_leaves_the_mapping_with_no_shape_clause`
/// pins that collapse and its tombstone says what it costs.
///
/// What survives is the guarantee this test was written for — a named document
/// that cannot answer says WHICH WAY it failed, and says it through the
/// accumulator — so the expected substrings are `unbound_shape_message`'s,
/// which name the document and the reason. The misspelt-shape row is gone with
/// the CURIE: a bare name that binds nothing is a name nobody declared, which
/// is a different sentence and the last case below.
#[test]
fn a_named_document_that_cannot_answer_says_which_way_it_failed() {
    use fossil_base::test_support::{PERSON_DOCUMENT, db_with_document};

    fn program(document: &str, shape: &str) -> String {
        format!(
            "type {{ T }} := io.shex(\"{document}\")\n\
             users := io.csv(\"x.csv\")\n\
             User : {shape} from users\n    \
             @subject = \"http://example.org/u/{{users.id}}\"\n    \
             name = users.name\n"
        )
    }

    // Each row: (the program, what is registered, its text, the substring the
    // message must carry).
    let cases: &[(String, &str, &str, &str)] = &[
        (
            program("missing.shex", "T"),
            "person.shex",
            PERSON_DOCUMENT,
            "its document `missing.shex` could not be read",
        ),
        (
            program("person.unknown", "T"),
            "person.unknown",
            PERSON_DOCUMENT,
            "its document `person.unknown` could not be read as a shape document",
        ),
        (
            program("broken.shex", "T"),
            "broken.shex",
            "!malformed expected a shape line\n",
            "expected a shape line",
        ),
        (
            program("person.shex", "Persn"),
            "person.shex",
            PERSON_DOCUMENT,
            "`Persn` is not a shape this program declares",
        ),
    ];

    for (src, path, text, expected) in cases {
        let (db, file) = db_with_document(src, path, text);
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let _ = typecheck_mapping(&db, m);
        let diags = crate::lower::lower_to_hir::accumulated::<Diagnostic>(&db, file);
        assert!(
            diags.iter().any(|d| d.message.contains(expected)),
            "expected a diagnostic containing {expected:?}, got {diags:#?}"
        );
    }
}

// ── Value-disjunction rejection diagnostic (SC#4) ──────────────────────────

/// The shape whose body is a disjunction the compiler does not support. The
/// decoder rejected it and named the branches; nothing here knows which schema
/// language it was written in, which is the point.
fn disjunction_rejection() -> Rejection {
    Rejection::Disjunction {
        shape_iri: "http://example.org/Person".to_string(),
        disjuncts: vec![
            vec!["http://example.org/email".to_string()],
            vec!["http://example.org/phone".to_string()],
        ],
    }
}

#[test]
fn a_disjunction_rejection_attaches_to_the_consuming_mapping() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<()> {
        let m = *def_map(db, file).mappings(db).first()?;
        let shape = ResolvedShape {
            constraints: Vec::new(),
            rejections: vec![disjunction_rejection()],
        };
        let mut cx = build_checker(db, m, None, Some(shape));
        cx.surface_shape_lowering_errors();
        Some(())
    }

    let (db, file) = db_with(HELLO);
    let _ = shim(&db, file);
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    let with_suggestion: Vec<_> = diags
        .iter()
        .filter(|d| d.suggestion_source.is_some())
        .collect();
    assert_eq!(
        with_suggestion.len(),
        1,
        "exactly one disjunction diagnostic carrying a structured \
         suggestion_source, got {} diagnostics: {diags:#?}",
        with_suggestion.len()
    );
    // Assert directly on the TYPED field — NOT the Markdown message.
    let suggestion = with_suggestion[0]
        .suggestion_source
        .as_deref()
        .expect("suggestion_source set");
    // HELLO is `User : Person from users` — the mapping is `User`, the
    // source binding is `users`, and telling them apart is the whole assertion.
    // Production passed the MAPPING name as the `from` clause, so it emitted
    // `User1 : … from User`: a `from` pointing at the mapping being split. This
    // test could not see it — it asserted `contains("from User")`, which the
    // wrong string satisfies and the right one (`from users`) does not, because
    // the capital is the only difference. The corpus could not see it either:
    // it calls `render_split_suggestion` directly and passed `"users"` by hand.
    assert!(
        suggestion.contains("User1 : ") && suggestion.contains("email = "),
        "split suggestion must be Fossil source naming the branch predicates, \
         got {suggestion:?}"
    );
    assert!(
        suggestion.contains("from users"),
        "the `from` clause is the SOURCE BINDING the mapping reads, \
         got {suggestion:?}"
    );
    assert!(
        !suggestion.contains("from User\n"),
        "a `from` naming the mapping itself is the bug this pins; \
         got {suggestion:?}"
    );
    assert!(
        with_suggestion[0].message.contains("disjunction"),
        "message must name the situation, got {:?}",
        with_suggestion[0].message
    );
}

/// The rendering the decoder used to own. It walked a cloned `OneOf` AST node
/// carried through the whole compiler for this one purpose; it now reads the
/// branch predicates, which is all it ever took out of that node.
///
/// The shape is a BARE NAME and the subject is an interpolated string, because
/// the arguments are what the caller has: `surface_shape_lowering_errors`
/// passes the mapping's own header and subject through verbatim.
///
/// # THIS TEST IS RED, AND IT IS THE RENDERER THAT IS WRONG
///
/// `render_split_suggestion` (`crate::check`, the `writeln!` in its branch
/// loop) emits `"    {short} = .{short}"`. A leading `.` is the retired
/// `FieldRef` and the parser refuses it, so the mapping it emits comes back
/// with one property where it wrote two — **the compiler emitting source it
/// cannot read back**, which is exactly what the renderer's own doc comment
/// promises it does not do, and what
/// `tests/diagnostic_corpus.rs::the_generated_split_suggestion_compiles` is
/// there to catch.
///
/// The expectation below is the CORRECT output — the `from` clause is the
/// binding a body's references are qualified against, so `email` reads
/// `users.email`. Rewriting it to match what the function does would make a
/// test fixture decide that the defect is the contract, and it is not a fixture
/// rewrite's call to make. Left failing on purpose; the repair is one line of
/// non-test code and belongs with whoever owns it.
// The `{users.id}` here is LITERAL Fossil source — the subject template the
// suggestion carries through — not a Rust format string.
#[allow(clippy::literal_string_with_formatting_args)]
#[test]
fn the_split_suggestion_is_one_mapping_per_branch() {
    let rendered = crate::check::render_split_suggestion(
        "Contact",
        "Contact",
        "users",
        "\"https://example.org/u/{users.id}\"",
        &[
            vec!["http://example.org/email".to_string()],
            vec!["http://example.org/phone".to_string()],
        ],
        &[],
    );
    assert_eq!(
        rendered,
        "Contact1 : Contact from users\n    \
         @subject = \"https://example.org/u/{users.id}\"\n    \
         email = users.email\n\n\
         Contact2 : Contact from users\n    \
         @subject = \"https://example.org/u/{users.id}\"\n    \
         phone = users.phone\n\n"
    );
}

/// A branch the decoder could not name is a comment, not an empty mapping body
/// the user has to notice is empty.
#[test]
fn a_branch_with_no_named_predicate_says_so() {
    let rendered =
        crate::check::render_split_suggestion("C", "C", "users", "\"t\"", &[Vec::new()], &[]);
    assert!(rendered.contains("# TODO"), "got {rendered:?}");
}

// ── Query-level inversions + contract ──────────────────────────────────────

#[test]
fn typecheck_mapping_returns_error_guaranteed_on_any_diagnostic() {
    // A mapping whose source has a row but references a missing column would
    // error. We use a direct Checker to force the error path and
    // assert the ErrorGuaranteed-implies-diagnostic contract.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let prop = HirProperty {
            key: PropertyKey::Name(smol_str::SmolStr::from("x")),
            value: HirExpr::FieldRef(smol_str::SmolStr::from("nonexistent")),
        };
        cx.check_property(ExprId(0), &prop);
        cx.expr.first_error.is_some()
    }

    let (db, file) = db_with(HELLO);
    assert!(shim(&db, file), "missing column must record an error");
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    assert!(!diags.is_empty(), "ErrorGuaranteed implies ≥1 Diagnostic");
}

#[test]
fn expr_types_is_thin_accessor_over_typecheck_mapping() {
    let (db, file) = db_with(HELLO);
    let m = first_mapping(&db, file);
    let via_accessor = expr_types(&db, m);
    let via_typeck = typecheck_mapping(&db, m)
        .expect("hello type-checks ok")
        .expr_types(&db);
    assert_eq!(
        via_accessor.entries(&db),
        via_typeck.entries(&db),
        "expr_types must project typecheck_mapping's expr_types verbatim"
    );
}

#[test]
fn typecheck_mapping_is_memoised() {
    let (db, file) = db_with(HELLO);
    let m = first_mapping(&db, file);
    let a = typecheck_mapping(&db, m);
    let b = typecheck_mapping(&db, m);
    assert_eq!(a, b);
}

// ── Row fixtures ──────────────────────────────────────────────────────────

/// Build a `Record` row `Ty` with the given `(name, primitive)` fields.
fn row_record<'db>(db: &'db dyn fossil_base::Db, fields: &[(&str, Primitive)]) -> Ty<'db> {
    let rec = crate::ty::Record::new(
        db,
        fields
            .iter()
            .map(|(n, p)| crate::ty::RecordField {
                name: smol_str::SmolStr::from(*n),
                ty: Ty::new(db, TyKind::Primitive(*p)),
            })
            .collect::<Vec<_>>(),
    );
    Ty::new(db, TyKind::Record(rec))
}

// The implicit-closure block lived here: `fn_over_row`, `closure_rendering`,
// `rewrite_field_refs_to_row_dot_edge_cases`,
// `expr_contains_free_field_refs_visits_all_arms`, and the six
// `closure_synth_*` tests. They were the ONLY callers of `Checker::check` and
// of `synthesize_closure`, which is what made both unreachable from
// `typecheck_mapping` — a whole algorithm proved by nothing but its own tests.
// `pipeline_typechecks_in_phase_3_v0_1` went with them: it asserted that
// `HELLO` type-checks, which `fieldref_without_schema_synthesises_no_type_phase_2_compat`
// already asserts, and its entire docblock was about the closure form.

// ── F2 §1/§2: calls and comparisons against the catalog and the row ────────

/// The mistake a mapping actually makes: comparing a text column with a number.
/// With a source row present, the checker has both types and refuses it.
#[test]
fn comparing_a_string_column_with_an_integer_is_an_error() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let e = crate::lower::HirExpr::BinOp {
            op: crate::lower::BinOp::Ge,
            lhs: Box::new(crate::lower::HirExpr::FieldRef("name".into())),
            rhs: Box::new(crate::lower::HirExpr::IntLit(18)),
        };
        let ty = cx.expr.synth(ExprId(0), &e)?;
        Some(render_ty_kind(db, ty.kind(db)))
    }

    let (db, file) = db_with(HELLO);
    let rendered = shim(&db, file).expect("synth returns a type");
    assert!(
        rendered.starts_with("Error"),
        "a String/Integer comparison must taint, got {rendered}"
    );
    let diags = shim::accumulated::<fossil_base::Diagnostic>(&db, file);
    assert!(
        diags.iter().any(|d| d.message.contains("cannot compare")),
        "the diagnostic must say what it could not compare, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
}

/// The same shape, well typed: an integer column against an integer literal.
#[test]
fn comparing_an_integer_column_with_an_integer_is_bool() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let e = crate::lower::HirExpr::BinOp {
            op: crate::lower::BinOp::Ge,
            lhs: Box::new(crate::lower::HirExpr::FieldRef("age".into())),
            rhs: Box::new(crate::lower::HirExpr::IntLit(18)),
        };
        let ty = cx.expr.synth(ExprId(0), &e)?;
        Some(render_ty_kind(db, ty.kind(db)))
    }

    let (db, file) = db_with(HELLO);
    assert_eq!(shim(&db, file).expect("synth"), "Bool");
}

/// A call's argument is checked against the declared parameter type, and the
/// result type is the signature's — not the argument's.
#[test]
fn a_call_takes_its_return_type_and_checks_its_argument() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<(String, String)> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        // `str.trim(String) -> String` applied to the String column: ok.
        let ok = crate::lower::HirExpr::Call {
            func: "str.trim".into(),
            args: vec![crate::lower::HirExpr::FieldRef("name".into())],
        };
        let ok_ty = cx.expr.synth(ExprId(0), &ok)?;
        // The same function applied to the Integer column: refused.
        let bad = crate::lower::HirExpr::Call {
            func: "str.trim".into(),
            args: vec![crate::lower::HirExpr::FieldRef("age".into())],
        };
        let bad_ty = cx.expr.synth(ExprId(0), &bad)?;
        Some((
            render_ty_kind(db, ok_ty.kind(db)),
            render_ty_kind(db, bad_ty.kind(db)),
        ))
    }

    let (db, file) = db_with(HELLO);
    let (ok, bad) = shim(&db, file).expect("synth");
    assert_eq!(ok, "String", "the return type is the signature's");
    assert!(
        bad.starts_with("Error"),
        "Integer is not a String, got {bad}"
    );
    let diags = shim::accumulated::<fossil_base::Diagnostic>(&db, file);
    // Argument 0 of a `Receiver::Scalar` row IS the receiver — `str.trim(x)`
    // and `x.trim()` are one row, and both put the value there — so the message
    // is about what the value IS, not about a position. This asserted
    // "argument 1 of `str.trim`", which is the OTHER branch of `synth_call`:
    // the one that fires for an argument the author actually wrote as one.
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("`trim` is a member of String")
                && d.message.contains("this is Integer")),
        "the diagnostic must name the member and both types, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
}

/// Two branches of different types is the error, not a widening. A column whose
/// type depends on the row is a column no shape can check.
#[test]
fn a_conditional_with_mismatched_branches_is_an_error() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let e = crate::lower::HirExpr::Ternary {
            cond: Box::new(crate::lower::HirExpr::BinOp {
                op: crate::lower::BinOp::Ge,
                lhs: Box::new(crate::lower::HirExpr::FieldRef("age".into())),
                rhs: Box::new(crate::lower::HirExpr::IntLit(18)),
            }),
            then: Box::new(crate::lower::HirExpr::StringLit("adult".into())),
            otherwise: Box::new(crate::lower::HirExpr::IntLit(0)),
        };
        let ty = cx.expr.synth(ExprId(0), &e)?;
        Some(render_ty_kind(db, ty.kind(db)))
    }

    let (db, file) = db_with(HELLO);
    assert!(
        shim(&db, file).expect("synth").starts_with("Error"),
        "String and Integer branches must not unify"
    );
    let diags = shim::accumulated::<fossil_base::Diagnostic>(&db, file);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("different types") && d.message.contains("does not coerce")),
        "the diagnostic must say both what differs and that fossil will not coerce, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
}

/// A non-Bool condition is refused before the branches are even considered.
#[test]
fn a_conditional_whose_condition_is_not_bool_is_an_error() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let e = crate::lower::HirExpr::Ternary {
            cond: Box::new(crate::lower::HirExpr::FieldRef("name".into())),
            then: Box::new(crate::lower::HirExpr::StringLit("a".into())),
            otherwise: Box::new(crate::lower::HirExpr::StringLit("b".into())),
        };
        let ty = cx.expr.synth(ExprId(0), &e)?;
        Some(render_ty_kind(db, ty.kind(db)))
    }

    let (db, file) = db_with(HELLO);
    assert!(shim(&db, file).expect("synth").starts_with("Error"));
    let diags = shim::accumulated::<fossil_base::Diagnostic>(&db, file);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("condition of `? :` must be Bool")),
        "got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
}

// ── `@rename` reaches every side that names a predicate (D2) ───────────────

/// A REQUIRED predicate the program renamed and then WROTE is accepted, and the
/// column the writer emits carries the same name.
///
/// Both halves are the defect, and they are one defect: `short_names` resolved
/// the body's keys rename-first, while `check_required_properties` walked the
/// shape's constraints with `local_name` and `OutputShapes::to_graph_schema`
/// emitted the column with `local_name`. So the program above — the only kind
/// `@rename` exists for — was told it had never written a property it had just
/// written, and had the compiler accepted it, the column would have shipped
/// under the name the rename renamed away from.
///
/// Asserting BOTH here is deliberate: either one alone passes with the other
/// still broken, because the two never meet at run time.
// The `{users.id}` hole is literal Fossil source, not a Rust format arg.
#[allow(clippy::literal_string_with_formatting_args)]
#[test]
fn a_renamed_required_property_is_written_and_shipped_under_the_rename() {
    const DOCUMENT: &str = "\
shape http://example.org/Person
prop http://example.org/name string 1 1
";
    const SRC: &str = "\
@rename(Person, \"http://example.org/name\" as full_name)
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"x.csv\")
User : Person from users
    @subject = \"http://example.org/u/{users.id}\"
    full_name = users.name
";

    let mut db = fossil_base::test_support::new_db();
    let file = SourceFile::new(&db, SRC.to_string(), "prog.fossil".to_string());
    fossil_base::test_support::register_document(&mut db, "person.shex", DOCUMENT);

    let mapping = first_mapping(&db, file);
    let _ = typecheck_mapping(&db, mapping);
    let messages: Vec<String> = typecheck_mapping::accumulated::<Diagnostic>(&db, mapping)
        .into_iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(
        !messages.iter().any(|m| m.contains("never writes")),
        "the required predicate was written, under the name the program renamed \
         it to; got {messages:?}"
    );

    let renames = def_map(&db, file).renames(&db);
    let document = fossil_base::file_at(&db, "person.shex").expect("registered above");
    let shapes = fossil_base::shape_document(&db, document, "shex").expect("the `shex` row");
    let schema = shapes.to_graph_schema(&renames);
    let columns: Vec<&str> = schema.nodes[0]
        .properties
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        columns,
        ["full_name"],
        "the writer must emit the column under the name the body wrote"
    );
}

// ── The reference type ─────────────────────────────────────────────────────

/// Two shapes and an edge between them, so the checker has something to be
/// wrong about.
///
/// `Order` declares `buyer @Person`, and both shapes mint an identity, which is
/// what an edge needs: a target whose `@subject` says how many holes to fill.
const TWO_SHAPES: &str = "\
shape https://example.org/Person
prop https://example.org/email string 1 1
shape https://example.org/Order
prop https://example.org/total float 1 1
prop https://example.org/buyer @https://example.org/Person 1 1
";

fn two_shape_program(edge_rhs: &str) -> String {
    format!(
        "type {{ Person, Order }} := io.shex(\"two.shex\")\n\
         U := io.csv(\"u.csv\")\n\
         P : Person from U\n    \
         @subject = \"https://example.org/p/{{U.email}}\"\n    \
         email    = U.email\n\
         O : Order from U\n    \
         @subject = \"https://example.org/o/{{U.email}}\"\n    \
         total    = 1.5\n    \
         buyer    = {edge_rhs}\n"
    )
}

/// Every diagnostic the CHECKER raises, over every mapping.
///
/// Gathered from `typecheck_mapping` and not from `lower_to_hir`: a Salsa
/// accumulator collects what a query and its dependencies pushed, and the
/// lowering does not depend on the checker. Reading the lowering's accumulator
/// for a type error yields an empty list, which reads exactly like «it
/// type-checked».
fn diagnostics_of(src: &str) -> Vec<String> {
    use fossil_base::test_support::db_with_document;
    let (db, file) = db_with_document(src, "two.shex", TWO_SHAPES);
    def_map(&db, file)
        .mappings(&db)
        .clone()
        .into_iter()
        .flat_map(|m| {
            typecheck_mapping::accumulated::<Diagnostic>(&db, m)
                .into_iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The check the language exists to make, and it did not happen.
///
/// `ex:buyer @ex:Person` says where the edge lands. While a reference was
/// `TyKind::Iri` every reference had ONE type, so `buyer = Order(…)` — an edge
/// to the wrong shape — type-checked clean, measured against
/// `apps/docs/programs/shop` on 2026-08-21.
///
/// Both halves are asserted, because a rule that refuses everything would pass
/// the first: the right edge is accepted and the wrong one is refused, naming
/// both shapes.
#[test]
fn an_edge_is_typed_by_the_shape_it_reaches() {
    let right = diagnostics_of(&two_shape_program("Person(U.email)"));
    assert!(
        right.is_empty(),
        "the edge the shape declares must type: {right:#?}"
    );

    let wrong = diagnostics_of(&two_shape_program("Order(U.email)"));
    assert!(
        wrong
            .iter()
            .any(|m| m.contains("Person") && m.contains("Order")),
        "an edge to the wrong shape must be refused, naming both: {wrong:#?}"
    );
}

/// A reference is a set, because `@<A> OR @<B>` is legal, and the subtyping is
/// set inclusion: a member type satisfies the union, as in `GraphQL`.
///
/// Asserted on the types directly. The surface has no spelling for a
/// disjunction-valued predicate that a program can write today — the decoder
/// rejects `OR` bodies — so a program-level version of this test would be
/// asserting on the rejection instead of on the rule.
#[test]
fn a_reference_to_one_shape_satisfies_a_slot_that_accepts_two() {
    use crate::ty::Ty;
    let (db, _) = db_with("");
    let a = SmolStr::new_static("https://example.org/A");
    let b = SmolStr::new_static("https://example.org/B");

    let one = Ty::reference(&db, [a.clone()]);
    let two = Ty::reference(&db, [a.clone(), b.clone()]);
    let other = Ty::reference(&db, [b.clone()]);

    assert!(crate::check::subtypes(&db, one, two), "A <: A|B");
    assert!(crate::check::subtypes(&db, other, two), "B <: A|B");
    assert!(!crate::check::subtypes(&db, two, one), "A|B is not <: A");
    assert!(!crate::check::subtypes(&db, one, other), "A is not <: B");

    // The set is the type: the order a document wrote it in is not part of it.
    assert_eq!(two, Ty::reference(&db, [b, a]), "canonicalised");
}

// ── The condition of a stage ───────────────────────────────────────────────

/// The defect this whole seam was built for, stated as the asymmetry it was.
///
/// `Row.celsius > "abc"` in a `where` and `parse.float(Row.celsius)` in a body
/// are the same mismatch on the same column. One was refused and the other
/// passed clean, because a stage's condition was walked for the column NAMES it
/// mentions (`infer::check_refs`) and never typed.
///
/// Both halves, because a rule that refuses everything would pass the first:
/// the well-typed comparison is accepted, the ill-typed one is refused, and the
/// unknown COLUMN is still refused — that is the question `check_refs` used to
/// answer, and `synth` of a `ColumnRef` answers it now.
#[test]
fn a_stage_condition_is_typed_and_not_only_resolved() {
    use fossil_base::test_support::{DecodingHost, register_inferred};
    use fossil_graph_schema::Primitive;
    use std::sync::Arc;

    fn diagnostics(condition: &str) -> Vec<String> {
        let system: Arc<dyn fossil_base::System> = Arc::new(DecodingHost::default());
        let db = FossilDb::new(system);
        register_inferred(
            &db,
            "r.csv",
            &[
                ("celsius", Primitive::Float),
                ("station", Primitive::String),
            ],
        );
        let src = format!("Row := io.csv(\"r.csv\")\nValid := Row.where({condition})\n");
        let file = SourceFile::new(&db, src, "stage.fossil".to_string());
        // `resolve_binding_scope` is a plain-Rust helper called from inside a
        // tracked frame, so the accumulator needs one — the same shim the row
        // algebra's own tests use.
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
            crate::infer::resolve_binding_scope(db, file, "Valid", 0).is_ok()
        }
        let _ = shim(&db, file);
        shim::accumulated::<Diagnostic>(&db, file)
            .into_iter()
            .map(|d| d.message.clone())
            .collect()
    }

    let ok = diagnostics("Row.celsius > 10.0");
    assert!(ok.is_empty(), "a well-typed condition must pass: {ok:#?}");

    let mistyped = diagnostics("Row.celsius > \"abc\"");
    assert!(
        mistyped
            .iter()
            .any(|m| m.contains("Float") && m.contains("String")),
        "comparing a Float with a String must be refused, naming both: {mistyped:#?}"
    );

    let unknown = diagnostics("Row.nosuch > 10.0");
    assert!(
        unknown.iter().any(|m| m.contains("nosuch")),
        "an unknown column is still refused: {unknown:#?}"
    );

    // A condition that is not a condition. The catalogue is what says so —
    // `seq.where` is `(Rows, Predicate) -> Rows` — where it used to say
    // `p("rows", S::String)`.
    let not_a_condition = diagnostics("Row.station");
    assert!(
        not_a_condition
            .iter()
            .any(|m| m.contains("needs a condition")),
        "a `where` over a String is not a filter: {not_a_condition:#?}"
    );
}

// ── `null` ─────────────────────────────────────────────────────────────────

/// Comparable with everything, assignable to nothing.
///
/// The rule lives in `synth_binop`'s `Eq`/`Ne` arm and NOT in `subtypes`, and
/// that placement is the whole of it. A bottom type that subtyped everything
/// would make `name = null` check against `xsd:string` — which is
/// `TyKind::Optional` coming back through the door it left by — and asking
/// whether a column has a value is not the same question as writing a property
/// from nothing.
#[test]
fn null_compares_with_anything_and_assigns_to_nothing() {
    use fossil_base::test_support::{DecodingHost, register_inferred};
    use fossil_graph_schema::Primitive;
    use std::sync::Arc;

    fn stage_diagnostics(condition: &str) -> Vec<String> {
        let system: Arc<dyn fossil_base::System> = Arc::new(DecodingHost::default());
        let db = FossilDb::new(system);
        register_inferred(
            &db,
            "r.csv",
            &[("when", Primitive::Date), ("n", Primitive::Float)],
        );
        let src = format!("Row := io.csv(\"r.csv\")\nValid := Row.where({condition})\n");
        let file = SourceFile::new(&db, src, "null.fossil".to_string());
        #[salsa::tracked]
        fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
            crate::infer::resolve_binding_scope(db, file, "Valid", 0).is_ok()
        }
        let _ = shim(&db, file);
        shim::accumulated::<Diagnostic>(&db, file)
            .into_iter()
            .map(|d| d.message.clone())
            .collect()
    }

    // Against a Date and against a Float — the two the `!= ""` idiom could not
    // reach, which is why it was only ever a test for STRINGS.
    for condition in [
        "Row.when == null",
        "Row.when != null",
        "Row.n != null",
        "null == Row.n",
    ] {
        let d = stage_diagnostics(condition);
        assert!(d.is_empty(), "`{condition}` must type: {d:#?}");
    }

    // And it is still a comparison: two things that do not compare, do not.
    let mistyped = stage_diagnostics("Row.when == Row.n");
    assert!(
        !mistyped.is_empty(),
        "`null` must not make every comparison legal"
    );
}

/// The other half, on the assignment side: a property written from `null`.
#[test]
fn a_property_written_from_null_is_refused() {
    let src = "\
type { T } := io.shex(\"person.shex\")
users := io.csv(\"x.csv\")
User : T from users
    @subject = \"http://example.org/u/{users.id}\"
    name = null
";
    // A shape that NARROWS. `PERSON_DOCUMENT` declares its predicate `-`, and
    // a shape that declines to narrow the value does not narrow it — so with
    // that one there is no expectation for `null` to fail, and the test would
    // be asserting the absence of a check rather than the presence of one.
    let (db, file) = fossil_base::test_support::db_with_document(
        src,
        "person.shex",
        "shape http://example.org/Person\nprop http://example.org/name string 1 1\n",
    );
    let m = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");
    let _ = typecheck_mapping(&db, m);
    let diags: Vec<String> = typecheck_mapping::accumulated::<Diagnostic>(&db, m)
        .into_iter()
        .map(|d| d.message.clone())
        .collect();
    assert!(
        diags.iter().any(|d| d.contains("Null")),
        "writing a property from `null` must be refused, naming the type: {diags:#?}"
    );
}
