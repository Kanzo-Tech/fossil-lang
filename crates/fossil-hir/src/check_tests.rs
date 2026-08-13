//! Integration tests for the Phase 3 bidirectional checker (plan 03-05).
//!
//! Covers: literal-subset regression (Phase 2 widening), forward CSVW
//! propagation (SC#1) + did-you-mean, backward cardinality blame against a
//! target shape (SC#2), the 5 subtyping rules, value-disjunction rejection
//! (SC#4), the `expr_types` thin-accessor inversion, and the
//! `ErrorGuaranteed`-implies-diagnostic contract.

use super::*;
use crate::body::ExprId;
use crate::def_map::{MappingLoc, def_map};
use crate::lower::{HirExpr, HirProperty, PropertyKey};
use crate::provenance::{expr_types, ty_origin};
use crate::shapes::ResolvedShape;
use crate::ty::{ShapeId, TyKind};
use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_graph_schema::{Occurs, Primitive};
use std::sync::Arc;

const HELLO: &str = "\
type { Person } := io.shex(\"personas.shex\")
User := io.csv(\"x.csv\")
Users : Person from User
    @subject = \"https://example.org/u/{User.id}\"
    name = User.name
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
/// It was a CSVW descriptor parsed by `CsvwDescriptor::parse`, and CSVW is
/// gone — it was deprecated by `D-CSVW-DEPRECATED`, which told the author to
/// remove the argument because types are inferred from the file directly. What
/// replaces it is the INFERRED descriptor: the shape a host registers after
/// introspecting the file, which is the direction the deprecation already
/// pointed at. Same three columns, same types, one less way to say it.
fn users_row<'db>(db: &'db dyn fossil_base::Db) -> Ty<'db> {
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
            freshness_token: String::new().into(),
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
            .map(|b| crate::infer::RowScope::one(b, source_row)),
        (scope, _) => scope,
    };
    Checker {
        db,
        mapping,
        source_row,
        source_scope,
        resolved_shape,
        predicates,
        spans: spans(db, mapping),
        entries: Vec::new(),
        next_inference: 0,
        first_error: None,
        iri_position: false,
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
    // Property 0 of hello is `iri = template` → IriTemplate. typecheck_mapping
    // records it; expr_types projects it (same as Phase 2's
    // expr_types_returns_iri_template_for_iri_property).
    let (db, file) = db_with(HELLO);
    let m = first_mapping(&db, file);
    let entry = ty_origin(&db, m, ExprId(0)).expect("property 0 (iri template) must have a type");
    assert_eq!(entry.ty.kind(&db), &TyKind::IriTemplate);
}

#[test]
fn fieldref_without_schema_synthesises_no_type_phase_2_compat() {
    // hello's `users` source has NO CSVW schema arg → source_row = None →
    // FieldRef `.name` synthesises no entry (preserving Phase 2's None, keeping
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

// ── Forward CSVW propagation (SC#1) ────────────────────────────────────────

#[test]
fn fieldref_csvw_propagates_string() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let ty = cx.lookup_field(ExprId(0), "name")?;
        Some(render_ty_kind(db, ty.kind(db)))
    }

    let (db, file) = db_with(HELLO);
    let rendered = shim(&db, file).expect("name lookup");
    assert_eq!(rendered, "String", ".name resolves to String via CSVW row");
}

#[test]
fn fieldref_csvw_typo_emits_did_you_mean() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<()> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = users_row(db);
        let mut cx = build_checker(db, m, Some(row), None);
        let _ = cx.lookup_field(ExprId(0), "naem"); // typo for `name`
        Some(())
    }

    let (db, file) = db_with(HELLO);
    let _ = shim(&db, file);
    let diags = about_the_body(&shim::accumulated::<Diagnostic>(&db, file));
    assert_eq!(
        diags.len(),
        1,
        "exactly one field-not-found diagnostic, got {diags:?}"
    );
    let msg = &diags[0];
    assert!(
        msg.contains("unknown column `naem`"),
        "diagnostic must name the unknown column, got {msg:?}"
    );
    assert!(
        msg.contains("did you mean `name`"),
        "diagnostic must suggest `name`, got {msg:?}"
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
        cx.first_error.is_some()
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
        cx.first_error.is_some()
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
        cx.first_error.is_some()
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
            &mut cx,
            int,
            Some(flt),
            ExprId(0),
            &BlamePos::Expr(ExprId(0)),
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
        compatible(&mut cx, s, Some(i), ExprId(0), &BlamePos::Expr(ExprId(0))).is_err()
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
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
Contact : ex:Person from users
    name = User.name
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
                shape_id: ShapeId::placeholder(0),
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
        cx.first_error.is_none()
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

// ── The four ways a named document fails to produce a shape ────────────────

/// Every one of these was a silent `None` — the same answer as "this program
/// names no document" — so the commonest mistake, a misspelt shape name,
/// produced no message at all. This drives the production query, so it also
/// proves the diagnostics reach the accumulator.
#[test]
fn a_named_document_that_cannot_answer_says_which_way_it_failed() {
    use fossil_base::test_support::{PERSON_DOCUMENT, db_with_document};

    fn program(document: &str, shape: &str) -> String {
        format!(
            "prefix ex: <http://example.org/>\n\
             type {{ T }} = io.shex(\"{document}\")\n\
             users := io.csv(\"x.csv\")\n\
             User : ex:{shape} from users\n    \
             name = User.name\n"
        )
    }

    // Each row: (the program, what is registered, its text, the substring the
    // message must carry).
    let cases: &[(String, &str, &str, &str)] = &[
        (
            program("person.shex", "Persn"),
            "person.shex",
            PERSON_DOCUMENT,
            "declares no shape `http://example.org/Persn` — did you mean",
        ),
        (
            program("missing.shex", "Person"),
            "person.shex",
            PERSON_DOCUMENT,
            "`missing.shex` is not there",
        ),
        (
            program("person.unknown", "Person"),
            "person.unknown",
            PERSON_DOCUMENT,
            "nothing here reads `person.unknown` as a shape document",
        ),
        (
            program("broken.shex", "Person"),
            "broken.shex",
            "!malformed expected a shape line\n",
            "did not parse: expected a shape line",
        ),
    ];

    for (src, path, text, expected) in cases {
        let (db, file) = db_with_document(src, path, text);
        let m = *def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("one mapping");
        let _ = typecheck_mapping(&db, m);
        let diags = typecheck_mapping::accumulated::<Diagnostic>(&db, m);
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
            shape_id: ShapeId::placeholder(0),
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
    // HELLO is `User : ex:Person from users` — the mapping is `User`, the
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
// The `${ex:}u/${.id}` here is LITERAL Fossil source — the subject template the
// suggestion carries through — not a Rust format string.
#[allow(clippy::literal_string_with_formatting_args)]
#[test]
fn the_split_suggestion_is_one_mapping_per_branch() {
    let rendered = crate::check::render_split_suggestion(
        "Contact",
        "ex:Contact",
        "users",
        "`${ex:}u/${.id}`",
        &[
            vec!["http://example.org/email".to_string()],
            vec!["http://example.org/phone".to_string()],
        ],
    );
    assert_eq!(
        rendered,
        "Contact1 : ex:Contact from users\n    \
         @subject = \"https://example.org/u/{User.id}\"\n    \
         email = .email\n\n\
         Contact2 : ex:Contact from users\n    \
         @subject = \"https://example.org/u/{User.id}\"\n    \
         phone = .phone\n\n"
    );
}

/// A branch the decoder could not name is a comment, not an empty mapping body
/// the user has to notice is empty.
#[test]
fn a_branch_with_no_named_predicate_says_so() {
    let rendered =
        crate::check::render_split_suggestion("C", "ex:C", "users", "`t`", &[Vec::new()]);
    assert!(rendered.contains("# TODO"), "got {rendered:?}");
}

// ── Query-level inversions + contract ──────────────────────────────────────

#[test]
fn typecheck_mapping_returns_error_guaranteed_on_any_diagnostic() {
    // A mapping whose source declares a CSVW schema but references a missing
    // column would error. We use a direct Checker to force the error path and
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
        cx.first_error.is_some()
    }

    let (db, file) = db_with(HELLO);
    assert!(shim(&db, file), "missing column must record an error");
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    assert!(
        !diags.is_empty(),
        "ErrorGuaranteed implies ≥1 Diagnostic (P-CRIT-4)"
    );
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
            op: crate::lower::CmpOp::Ge,
            lhs: Box::new(crate::lower::HirExpr::FieldRef("name".into())),
            rhs: Box::new(crate::lower::HirExpr::IntLit(18)),
        };
        let ty = cx.synth(ExprId(0), &e)?;
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
            op: crate::lower::CmpOp::Ge,
            lhs: Box::new(crate::lower::HirExpr::FieldRef("age".into())),
            rhs: Box::new(crate::lower::HirExpr::IntLit(18)),
        };
        let ty = cx.synth(ExprId(0), &e)?;
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
        let ok_ty = cx.synth(ExprId(0), &ok)?;
        // The same function applied to the Integer column: refused.
        let bad = crate::lower::HirExpr::Call {
            func: "str.trim".into(),
            args: vec![crate::lower::HirExpr::FieldRef("age".into())],
        };
        let bad_ty = cx.synth(ExprId(0), &bad)?;
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
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("argument 1 of `str.trim`")),
        "the diagnostic must name the argument and the function, got: {:?}",
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
                op: crate::lower::CmpOp::Ge,
                lhs: Box::new(crate::lower::HirExpr::FieldRef("age".into())),
                rhs: Box::new(crate::lower::HirExpr::IntLit(18)),
            }),
            then: Box::new(crate::lower::HirExpr::StringLit("adult".into())),
            otherwise: Box::new(crate::lower::HirExpr::IntLit(0)),
        };
        let ty = cx.synth(ExprId(0), &e)?;
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
        let ty = cx.synth(ExprId(0), &e)?;
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
