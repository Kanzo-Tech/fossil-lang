//! Integration tests for the Phase 3 bidirectional checker (plan 03-05).
//!
//! Covers: literal-subset regression (Phase 2 widening), forward CSVW
//! propagation (SC#1) + did-you-mean, backward `ShEx` cardinality blame
//! (SC#2), the 5 subtyping rules, `ShEx` `OneOf` rejection diagnostic emission
//! (SC#4), the `expr_types` thin-accessor inversion, and the
//! `ErrorGuaranteed`-implies-diagnostic contract.

use super::*;
use crate::body::ExprId;
use crate::def_map::{MappingLoc, def_map};
use crate::infer::record_from_descriptor;
use crate::lower::{HirExpr, HirProperty, PropertyKey};
use crate::provenance::{expr_types, ty_origin};
use crate::shapes::ResolvedShape;
use crate::ty::{ShapeId, TyKind};
use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_input::CsvwDescriptor;
use fossil_graph_schema::Primitive;
use std::sync::Arc;

const HELLO: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
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

const USERS_CSVW: &str = r#"{
  "@context": "http://www.w3.org/ns/csvw",
  "tableSchema": {
    "columns": [
      { "name": "id", "datatype": "integer" },
      { "name": "name", "datatype": "string" },
      { "name": "age", "datatype": "integer" }
    ]
  }
}"#;

/// Build a `Checker` over a fixture, with an explicit source row + optional
/// resolved shape, inside a tracked shim so `delay_span_bug` is valid.
fn build_checker<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    source_row: Option<Ty<'db>>,
    resolved_shape: Option<ResolvedShape<'db>>,
) -> Checker<'db> {
    Checker {
        db,
        mapping,
        source_row,
        resolved_shape,
        spans: spans(db, mapping),
        entries: Vec::new(),
        next_inference: 0,
        first_error: None,
    }
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
        let mut cx = build_checker(db, m, Some(row), None);
        let _ = cx.lookup_field(ExprId(0), "naem"); // typo for `name`
        Some(())
    }

    let (db, file) = db_with(HELLO);
    let _ = shim(&db, file);
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    assert_eq!(diags.len(), 1, "exactly one field-not-found diagnostic");
    let msg = &diags[0].message;
    assert!(
        msg.contains("unknown column `naem`"),
        "diagnostic must name the unknown column, got {msg:?}"
    );
    assert!(
        msg.contains("did you mean `name`"),
        "diagnostic must suggest `name`, got {msg:?}"
    );
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
            flt,
            Cardinality::Exact(1),
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
        compatible(
            &mut cx,
            s,
            i,
            Cardinality::Exact(1),
            ExprId(0),
            &BlamePos::Expr(ExprId(0)),
        )
        .is_err()
    }

    let (db, file) = db_with(HELLO);
    assert!(shim(&db, file), "String is not a subtype of Integer");
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    assert_eq!(diags.len(), 1);
    let msg = &diags[0].message;
    assert!(
        msg.contains("String") && msg.contains("Integer"),
        "got {msg:?}"
    );
}

#[test]
fn compatible_string_lifts_into_optional() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        let s = Ty::new(db, TyKind::Primitive(Primitive::String));
        let opt_s = Ty::new(db, TyKind::Optional(s));
        let mut cx = build_checker(db, m, None, None);
        // S-Opt: String <: Optional<String>, cardinality ZeroOrOne.
        compatible(
            &mut cx,
            s,
            opt_s,
            Cardinality::ZeroOrOne,
            ExprId(0),
            &BlamePos::Expr(ExprId(0)),
        )
        .is_ok()
    }

    let (db, file) = db_with(HELLO);
    assert!(
        shim(&db, file),
        "String <: Optional<String> (S-Opt) must hold"
    );
}

// ── Backward ShEx cardinality blame (SC#2) ─────────────────────────────────

#[test]
fn compatible_optional_fails_against_cardinality_one_required() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<(u32, u32)> {
        let m = *def_map(db, file).mappings(db).first()?;
        let s = Ty::new(db, TyKind::Primitive(Primitive::String));
        let opt_s = Ty::new(db, TyKind::Optional(s));
        let mut cx = build_checker(db, m, None, None);
        // Optional<String> provided where the shape demands OneOrMore → Err.
        let r = compatible(
            &mut cx,
            opt_s,
            s,
            Cardinality::OneOrMore,
            ExprId(0),
            &BlamePos::Expr(ExprId(0)),
        );
        assert!(r.is_err(), "Optional<X> vs cardinality 1+ must error");
        // Return the source span so the test asserts it is real (non-zero).
        let sp = cx.span_of(ExprId(0));
        Some((sp.start, sp.end))
    }

    let (db, file) = db_with(HELLO);
    let (start, end) = shim(&db, file).expect("Err path taken");
    // ExprId(0) is the iri-template property whose span is real.
    assert!(
        end > start,
        "source span must be real (non-zero), got {start}..{end}"
    );
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    assert_eq!(diags.len(), 1);
    assert!(
        diags[0].message.contains("cardinality 1+"),
        "diagnostic must mention the cardinality demand, got {:?}",
        diags[0].message
    );
    // Dest span embedded in the message must be non-zero too (both sides real).
    assert!(
        !diags[0].message.contains("start: 0, end: 0"),
        "dest span in the blame must be real, got {:?}",
        diags[0].message
    );
}

#[test]
fn check_property_against_shex_shape_optional_fails() {
    // End-to-end: a mapping with `ex:email = .email` where the source row types
    // `email` as Optional<String> but the ShEx shape demands cardinality 1+.
    const SRC: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
Contact : ex:Person from users
    ex:email = .email
";
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> bool {
        let Some(m) = def_map(db, file).mappings(db).first().copied() else {
            return false;
        };
        // Source row: email : Optional<String>.
        let s = Ty::new(db, TyKind::Primitive(Primitive::String));
        let opt = Ty::new(db, TyKind::Optional(s));
        let rec = crate::ty::Record::new(
            db,
            vec![crate::ty::RecordField {
                name: smol_str::SmolStr::from("email"),
                ty: opt,
            }],
        );
        let row = Ty::new(db, TyKind::Record(rec));
        // Resolved shape: predicate ex:email with String, OneOrMore.
        let shape = ResolvedShape {
            shape_id: ShapeId::placeholder(0),
            constraints: vec![crate::shapes::ShapeConstraint {
                predicate: smol_str::SmolStr::from("https://example.org/email"),
                value_ty: Some(s),
                cardinality: Cardinality::OneOrMore,
            }],
            errors: Vec::new(),
        };
        let mut cx = build_checker(db, m, Some(row), Some(shape));
        let prop = HirProperty {
            key: PropertyKey::PrefixedName {
                iri: smol_str::SmolStr::from("https://example.org/email"),
            },
            value: HirExpr::FieldRef(smol_str::SmolStr::from("email")),
        };
        cx.check_property(ExprId(0), &prop);
        cx.first_error.is_some()
    }

    let (db, file) = db_with(SRC);
    assert!(
        shim(&db, file),
        "Optional<String> vs cardinality 1+ must error"
    );
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    assert!(!diags.is_empty(), "must emit a backward-check diagnostic");
}

// ── ShEx OneOf rejection diagnostic (SC#4) ─────────────────────────────────

const ONE_OF_SCHEMA: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "OneOf",
          "expressions": [
            { "type": "TripleConstraint", "predicate": "http://example.org/email" },
            { "type": "TripleConstraint", "predicate": "http://example.org/phone" }
          ]
        }
      }
    }
  ]
}"#;

#[test]
fn shex_one_of_rejection_attaches_to_consuming_mapping() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<()> {
        let m = *def_map(db, file).mappings(db).first()?;
        let descriptor =
            fossil_descriptors_output::ShExDescriptor::from_reader(ONE_OF_SCHEMA.as_bytes())
                .ok()?;
        let errors = descriptor.lowering_errors().to_vec();
        assert!(
            !errors.is_empty(),
            "OneOf schema must produce a lowering error"
        );
        let binding = descriptor
            .shapes()
            .next()
            .cloned()
            .unwrap_or_else(|| panic!("at least one shape"));
        let shape = ResolvedShape::from_binding(db, &binding, ShapeId::placeholder(0), errors);
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
        "exactly one OneOf rejection diagnostic carrying a structured \
         suggestion_source, got {} diagnostics: {diags:#?}",
        with_suggestion.len()
    );
    // Assert directly on the TYPED field — NOT the Markdown message.
    let suggestion = with_suggestion[0]
        .suggestion_source
        .as_deref()
        .expect("suggestion_source set");
    assert!(
        suggestion.contains("from users") || suggestion.contains(':'),
        "split suggestion must be Fossil source, got {suggestion:?}"
    );
    // The message names the OneOf situation.
    assert!(
        with_suggestion[0].message.contains("OneOf"),
        "message must mention OneOf, got {:?}",
        with_suggestion[0].message
    );
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).expect("csvw");
        let row = record_from_descriptor(db, &descriptor, "users");
        let mut cx = build_checker(db, m, Some(row), None);
        let prop = HirProperty {
            key: PropertyKey::PrefixedName {
                iri: smol_str::SmolStr::from("https://example.org/x"),
            },
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

// ── CORE-07: implicit closure synthesis (plan 03-06) ───────────────────────

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

/// `Fn(Record<R> -> τ)` expected type.
fn fn_over_row<'db>(db: &'db dyn fossil_base::Db, row: Ty<'db>, ret: Ty<'db>) -> Ty<'db> {
    let sig = crate::ty::FnSig::new(db, vec![row], ret);
    Ty::new(db, TyKind::Fn(sig))
}

/// The `SynthesizedClosureRendering` rendering recorded on `expr_id`, if any.
fn closure_rendering(cx: &Checker<'_>, expr_id: ExprId) -> Option<String> {
    cx.entries.iter().rev().find_map(|e| {
        if e.expr_id == expr_id
            && let ProvenanceKind::SynthesizedClosureRendering { rendering } = &e.provenance.kind
        {
            return Some(rendering.to_string());
        }
        None
    })
}

#[test]
fn rewrite_field_refs_to_row_dot_edge_cases() {
    // Bare single field ref.
    assert_eq!(rewrite_field_refs_to_row_dot(".age"), "row.age");
    // Binop predicate: only the `.age` gets rewritten, the literal `18` doesn't.
    assert_eq!(rewrite_field_refs_to_row_dot(".age >= 18"), "row.age >= 18");
    // A decimal point inside a number must NOT be rewritten (prev char is a
    // digit → blocked).
    assert_eq!(rewrite_field_refs_to_row_dot("3.14"), "3.14");
    // Leading whitespace + two field refs in a comparison.
    assert_eq!(rewrite_field_refs_to_row_dot(".x > .y"), "row.x > row.y");
    // Field ref at expression start preceded by `(`.
    assert_eq!(rewrite_field_refs_to_row_dot("(.id)"), "(row.id)");
}

#[test]
fn expr_contains_free_field_refs_visits_all_arms() {
    // FieldRef → true.
    assert!(expr_contains_free_field_refs(&HirExpr::FieldRef(
        smol_str::SmolStr::from("age")
    )));
    // Non-FieldRef leaf forms → false (no row dependency).
    assert!(!expr_contains_free_field_refs(&HirExpr::StringLit(
        smol_str::SmolStr::from("hi")
    )));
    assert!(!expr_contains_free_field_refs(&HirExpr::Template(
        smol_str::SmolStr::from("`x`")
    )));
    assert!(!expr_contains_free_field_refs(&HirExpr::PrefixedName {
        iri: smol_str::SmolStr::from("https://example.org/Foo"),
    }));
}

#[test]
fn closure_synth_on_simple_field_ref() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = row_record(db, &[("id", Primitive::String)]);
        let str_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
        let expected = fn_over_row(db, row, str_ty);
        let mut cx = build_checker(db, m, Some(row), None);
        let arg = HirExpr::FieldRef(smol_str::SmolStr::from("id"));
        let result = cx.check(
            ExprId(0),
            &arg,
            expected,
            Cardinality::Exact(1),
            &BlamePos::Expr(ExprId(0)),
        );
        assert!(result.is_ok(), "closure synth over `.id` must check OK");
        closure_rendering(&cx, ExprId(0))
    }

    let (db, file) = db_with(HELLO);
    let rendering = shim(&db, file).expect("closure rendering must be recorded");
    assert_eq!(rendering, "(row: Record<{id: String}>) => row.id");
}

#[test]
fn closure_synth_on_binop_predicate() {
    // SC#3-shape: the canonical `users |> filter(.age >= 18)` predicate body.
    // Phase 3 v0.1 has no surface binop HIR node, so we drive synthesis with a
    // FieldRef arg and assert the rendering for the FieldRef form; the binop
    // text form is exercised via render_closure directly below.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = row_record(db, &[("age", Primitive::Integer)]);
        let int_ty = Ty::new(db, TyKind::Primitive(Primitive::Integer));
        // The closure body `.age` resolves to Integer; expected predicate
        // result Integer. (Bool comparison ops are a later phase; the row
        // context resolution + rendering is what CORE-07 validates here.)
        let expected = fn_over_row(db, row, int_ty);
        let mut cx = build_checker(db, m, Some(row), None);
        let arg = HirExpr::FieldRef(smol_str::SmolStr::from("age"));
        let _ = cx.check(
            ExprId(0),
            &arg,
            expected,
            Cardinality::Exact(1),
            &BlamePos::Expr(ExprId(0)),
        );
        closure_rendering(&cx, ExprId(0))
    }

    let (db, file) = db_with(HELLO);
    let rendering = shim(&db, file).expect("closure rendering recorded");
    assert_eq!(rendering, "(row: Record<{age: Integer}>) => row.age");

    // The SC#3 acceptance shape — `(row: Record<{age: Integer}>) => row.age >= 18`
    // — via render_closure directly (the binop text form), proving the renderer
    // produces the documented SC#3 string once a binop HIR node exists.
    let (db2, _f2) = db_with(HELLO);
    let row = row_record(&db2, &[("age", Primitive::Integer)]);
    let sc3 = render_closure(&db2, row, ".age >= 18");
    assert_eq!(
        sc3.as_str(),
        "(row: Record<{age: Integer}>) => row.age >= 18"
    );
}

#[test]
fn closure_synth_multi_field_record_rendering() {
    // A multi-field row renders every field name + type in declaration order.
    let (db, _file) = db_with(HELLO);
    let row = row_record(
        &db,
        &[
            ("id", Primitive::String),
            ("name", Primitive::String),
            ("age", Primitive::Integer),
        ],
    );
    let rendering = render_closure(&db, row, ".age");
    assert_eq!(
        rendering.as_str(),
        "(row: Record<{id: String, name: String, age: Integer}>) => row.age"
    );
}

#[test]
fn no_closure_synth_when_no_free_field_refs() {
    // A function-arg position expecting Fn(Record<{}> -> Iri) whose arg is a
    // PrefixedName (no FieldRef) must NOT synthesise a closure — it falls
    // through to the standard synth + compatible path, so no
    // SynthesizedClosureRendering entry is recorded.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<bool> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = row_record(db, &[]);
        let iri_ty = Ty::new(db, TyKind::Iri);
        let expected = fn_over_row(db, row, iri_ty);
        let mut cx = build_checker(db, m, Some(row), None);
        let arg = HirExpr::PrefixedName {
            iri: smol_str::SmolStr::from("https://example.org/Foo"),
        };
        let _ = cx.check(
            ExprId(0),
            &arg,
            expected,
            Cardinality::Exact(1),
            &BlamePos::Expr(ExprId(0)),
        );
        Some(closure_rendering(&cx, ExprId(0)).is_some())
    }

    let (db, file) = db_with(HELLO);
    let synthesised = shim(&db, file).expect("shim ran");
    assert!(
        !synthesised,
        "no FieldRef → no implicit closure synthesis (standard check path)"
    );
}

#[test]
fn closure_synth_with_missing_field_emits_did_you_mean() {
    // A `.naem` typo inside the closure body emits the did-you-mean diagnostic
    // (against the row's ACTUAL fields), AND the closure rendering is still
    // produced (synthesis does not abort on a body error).
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = row_record(db, &[("name", Primitive::String)]);
        let bool_ty = Ty::new(db, TyKind::Primitive(Primitive::Bool));
        let expected = fn_over_row(db, row, bool_ty);
        let mut cx = build_checker(db, m, Some(row), None);
        let arg = HirExpr::FieldRef(smol_str::SmolStr::from("naem")); // typo
        let _ = cx.check(
            ExprId(0),
            &arg,
            expected,
            Cardinality::Exact(1),
            &BlamePos::Expr(ExprId(0)),
        );
        closure_rendering(&cx, ExprId(0))
    }

    let (db, file) = db_with(HELLO);
    let rendering = shim(&db, file).expect("rendering still produced on body error");
    // Rendering uses the row's real field (`name`), not the typo'd `.naem`.
    assert_eq!(rendering, "(row: Record<{name: String}>) => row.naem");
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    let msg = diags
        .iter()
        .map(|d| d.message.as_str())
        .find(|m| m.contains("naem"))
        .expect("did-you-mean diagnostic for the typo");
    assert!(
        msg.contains("unknown column `naem`") && msg.contains("did you mean `name`"),
        "missing-field inside closure must still fire did-you-mean, got {msg:?}"
    );
}

#[test]
fn closure_synth_rendering_never_contains_unknown_or_inferenceid() {
    // Risk Register: the closure renderer must never leak the internal
    // TyKind::Unknown(InferenceId) placeholder. Build a row whose field is
    // explicitly Unknown(InferenceId) and assert it renders as `?`, not the
    // Debug form.
    let (db, _file) = db_with(HELLO);
    let unknown = Ty::new(&db, TyKind::Unknown(crate::ty::InferenceId(7)));
    let rec = crate::ty::Record::new(
        &db,
        vec![crate::ty::RecordField {
            name: smol_str::SmolStr::from("mystery"),
            ty: unknown,
        }],
    );
    let row = Ty::new(&db, TyKind::Record(rec));
    let rendering = render_closure(&db, row, ".mystery");
    assert!(
        !rendering.contains("Unknown"),
        "rendering must NOT leak `Unknown`, got {rendering:?}"
    );
    assert!(
        !rendering.contains("InferenceId"),
        "rendering must NOT leak `InferenceId`, got {rendering:?}"
    );
    assert_eq!(
        rendering.as_str(),
        "(row: Record<{mystery: ?}>) => row.mystery",
        "internal inference state normalises to `?` at the display boundary"
    );
}

#[test]
fn closure_synth_type_mismatch_emits_diagnostic() {
    // A closure body whose type does not satisfy the predicate result type
    // emits a standard mismatch diagnostic (synthesis does not swallow errors).
    // `.id : String` checked against expected predicate result `Integer` →
    // String ≮: Integer.
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<bool> {
        let m = *def_map(db, file).mappings(db).first()?;
        let row = row_record(db, &[("id", Primitive::String)]);
        let int_ty = Ty::new(db, TyKind::Primitive(Primitive::Integer));
        let expected = fn_over_row(db, row, int_ty);
        let mut cx = build_checker(db, m, Some(row), None);
        let arg = HirExpr::FieldRef(smol_str::SmolStr::from("id")); // String
        let _ = cx.check(
            ExprId(0),
            &arg,
            expected,
            Cardinality::Exact(1),
            &BlamePos::Expr(ExprId(0)),
        );
        Some(cx.first_error.is_some())
    }

    let (db, file) = db_with(HELLO);
    let errored = shim(&db, file).expect("shim ran");
    assert!(errored, "String body vs Integer predicate must error");
    let diags = shim::accumulated::<Diagnostic>(&db, file);
    let msg = diags
        .iter()
        .map(|d| d.message.as_str())
        .find(|m| m.contains("String") && m.contains("Integer"))
        .expect("type mismatch diagnostic inside closure body");
    assert!(
        msg.contains("expected `Integer`") && msg.contains("got `String`"),
        "closure body type mismatch must surface, got {msg:?}"
    );
}

#[test]
fn pipeline_typechecks_in_phase_3_v0_1() {
    // Phase 3 v0.1's HirExpr is the Phase 2 leaf surface (Template / FieldRef /
    // StringLit / PrefixedName) — there is NO Pipeline/Call form to lower yet,
    // so `users |> seq.filter(...)` cannot be expressed. This test documents
    // that a plain mapping with no closure-triggering form type-checks; the
    // seq.filter stub decision (NOT added — see 03-05-SUMMARY.md) is recorded
    // for plan 03-07's SC#3 LSP smoke test strategy.
    let (db, file) = db_with(HELLO);
    let m = first_mapping(&db, file);
    assert!(
        typecheck_mapping(&db, m).is_ok(),
        "hello (no closure form) type-checks ok"
    );
}

// ── F2 §1/§2: calls and comparisons against the catalog and the row ────────

/// The mistake a mapping actually makes: comparing a text column with a number.
/// With a source row present, the checker has both types and refuses it.
#[test]
fn comparing_a_string_column_with_an_integer_is_an_error() {
    #[salsa::tracked]
    fn shim(db: &dyn fossil_base::Db, file: SourceFile) -> Option<String> {
        let m = *def_map(db, file).mappings(db).first()?;
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
        let mut cx = build_checker(db, m, Some(row), None);
        // `clean.trim(String) -> String` applied to the String column: ok.
        let ok = crate::lower::HirExpr::Call {
            func: "clean.trim".into(),
            args: vec![crate::lower::HirExpr::FieldRef("name".into())],
        };
        let ok_ty = cx.synth(ExprId(0), &ok)?;
        // The same function applied to the Integer column: refused.
        let bad = crate::lower::HirExpr::Call {
            func: "clean.trim".into(),
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
            .any(|d| d.message.contains("argument 1 of `clean.trim`")),
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
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
        let descriptor = CsvwDescriptor::parse(USERS_CSVW.as_bytes()).ok()?;
        let row = record_from_descriptor(db, &descriptor, "users");
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
