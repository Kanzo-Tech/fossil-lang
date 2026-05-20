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
use crate::ty::{Primitive, ShapeId, TyKind};
use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_descriptors_input::CsvwDescriptor;
use std::sync::Arc;

const HELLO: &str = "\
prefix ex: <https://example.org/>
users := io.csv(\"x.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

fn db_with(src: &str) -> (FossilDb, SourceFile) {
    let system: Arc<dyn System> = Arc::new(NativeSystem);
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
