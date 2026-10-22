//! **What a declared type constrains beyond its fields' types** — `@assert`,
//! `@severity` and `Seq<T, n, m>`, one titled test per claim of
//! `/docs/design/declared`.
//!
//! # What it does NOT prove
//!
//! - **That a run checks what the program cannot.** A value read from a column
//!   is the run's to check; these tests hold the checker's half alone.

use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_base::{Diagnostic, FossilDb, SourceFile};
use fossil_graph_schema::{ConstraintSeverity, Occurs, Primitive, Problem, Severity};
use fossil_hir::def_map::def_map;
use fossil_hir::lower::lower_to_hir;
use fossil_hir::typecheck_mapping;

const COLUMNS: &[(&str, Primitive)] = &[
    ("id", Primitive::Integer),
    ("age", Primitive::Integer),
    ("mail", Primitive::String),
];

/// The constraints design's declaration: an assertion on the type, one on a
/// field with a severity, a field's `@severity`, and a `Seq` with both bounds.
const PERSONA: &str = "\
@iri(\"https://schema.org/Person\")
@assert(alta <= baja)
type Persona
    @iri(\"https://schema.org/name\")   nombre: String

    @iri(\"https://schema.org/age\")
    @assert(edad >= 0 and edad < 150)
    edad: Option<Integer>

    @iri(\"https://schema.org/email\")
    @assert(str.matches(email, \"^[^@]+@[^@]+$\"), severity = warning)
    @severity(info)
    email: Seq<String, 1, 3>

    @iri(\"https://ex.org/alta\")       alta: Date
    @iri(\"https://ex.org/baja\")       baja: Option<Date>

rows := io.csv(\"u.csv\")
";

fn open(src: &str) -> (FossilDb, SourceFile) {
    let (mut db, file) = db_with_document(src, "unused.shex", "");
    register_inferred(&mut db, "u.csv", COLUMNS);
    (db, file)
}

/// Every diagnostic the program produces: the parse's, the file's, the
/// assertions', and each mapping's body and check.
fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let (db, file) = open(src);
    let mut out: Vec<Diagnostic> = fossil_syntax::parse(&db, file).diagnostics(&db).clone();
    out.extend(lower_to_hir(&db, file).diagnostics(&db).iter().cloned());
    out.extend(
        fossil_hir::asserts::check_asserts(&db, file)
            .iter()
            .cloned(),
    );
    for m in def_map(&db, file).mappings(&db) {
        out.extend(
            fossil_hir::body::body(&db, *m)
                .diagnostics(&db)
                .iter()
                .cloned(),
        );
        out.extend(typecheck_mapping(&db, *m).diagnostics(&db).iter().cloned());
    }
    out
}

fn problems(src: &str) -> Vec<Problem> {
    diagnostics(src).into_iter().map(|d| d.problem).collect()
}

/// Exactly one diagnostic matching `want`, so a test cannot pass by finding it
/// in a list of twenty.
fn the_one(src: &str, want: impl Fn(&Problem) -> bool) -> Diagnostic {
    let all = diagnostics(src);
    let hits: Vec<&Diagnostic> = all.iter().filter(|d| want(&d.problem)).collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one, got {}:\n{all:#?}",
        hits.len()
    );
    hits[0].clone()
}

/// A one-field type, so a test reads as the one line it is about.
fn one_field(field: &str) -> String {
    format!(
        "@iri(\"https://e.org/T\")\ntype T\n    @iri(\"https://e.org/a\")\n    {field}\n\n\
         rows := io.csv(\"u.csv\")\n"
    )
}

const PERSONA_MAPPING: &str = "
People : Persona from rows
    @subject = \"https://e.org/p/{rows.id}\"
    nombre   = \"x\"
    email    = rows.mail
    alta     = parse.date(\"2020-01-01\", \"%Y-%m-%d\")
";

#[test]
fn the_design_declaration_is_clean() {
    let src = format!("{PERSONA}{PERSONA_MAPPING}");
    assert_eq!(problems(&src), Vec::<Problem>::new());
}

#[test]
fn bounds_are_the_count_of_a_seq() {
    let (db, file) = open(&format!("{PERSONA}{PERSONA_MAPPING}"));
    let hir = lower_to_hir(&db, file);
    let shape = hir.declared(&db).shapes().next().expect("one shape");
    let occurs: Vec<(&str, Occurs)> = shape
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.occurs))
        .collect();
    assert_eq!(
        occurs,
        [
            ("nombre", Occurs::ONE),
            (
                "edad",
                Occurs {
                    min: 0,
                    max: Some(1)
                }
            ),
            (
                "email",
                Occurs {
                    min: 1,
                    max: Some(3)
                }
            ),
            ("alta", Occurs::ONE),
            (
                "baja",
                Occurs {
                    min: 0,
                    max: Some(1)
                }
            ),
        ]
    );
    let at_least = open(&one_field("a: Seq<String, 2>"));
    let hir = lower_to_hir(&at_least.0, at_least.1);
    let shape = hir
        .declared(&at_least.0)
        .shapes()
        .next()
        .expect("one shape");
    assert_eq!(shape.properties[0].occurs, Occurs { min: 2, max: None });
}

#[test]
fn a_count_another_spelling_says_or_no_field_can_hold_is_refused() {
    for written in [
        "Seq<String, 0, 1>",
        "Seq<String, 1, 1>",
        "Seq<String, 0>",
        "Seq<String, 3, 1>",
        "Seq<String, 0, 0>",
        "Option<String, 2>",
    ] {
        let src = one_field(&format!("a: {written}"));
        let all = problems(&src);
        assert!(
            matches!(all.as_slice(), [Problem::InvalidFieldType { written: w, .. }] if w == written),
            "{written}: {all:#?}"
        );
    }
}

#[test]
fn asserts_and_severities_lower_beside_the_shape() {
    let (db, file) = open(PERSONA);
    let rules = lower_to_hir(&db, file).rules(&db).clone();
    let asserts: Vec<(Option<&str>, &str, ConstraintSeverity)> = rules
        .asserts
        .iter()
        .map(|a| (a.field.as_deref(), a.text.as_str(), a.severity))
        .collect();
    assert_eq!(
        asserts,
        [
            (None, "alta <= baja", ConstraintSeverity::Violation),
            (
                Some("edad"),
                "edad >= 0 and edad < 150",
                ConstraintSeverity::Violation
            ),
            (
                Some("email"),
                "str.matches(email, \"^[^@]+@[^@]+$\")",
                ConstraintSeverity::Warning
            ),
        ]
    );
    let counts: Vec<(&str, ConstraintSeverity)> = rules
        .counts
        .iter()
        .map(|c| (c.field.as_str(), c.severity))
        .collect();
    assert_eq!(counts, [("email", ConstraintSeverity::Info)]);
}

#[test]
fn an_assertion_is_a_bool() {
    let d = the_one(&one_field("@assert(str.length(a))\n    a: String"), |p| {
        matches!(p, Problem::ExpectedBool { .. })
    });
    assert!(
        matches!(&d.problem, Problem::ExpectedBool { operand, .. } if operand == "the assertion on the field `a`"),
        "{d:#?}"
    );
}

#[test]
fn an_assertion_on_a_field_reads_that_field_alone() {
    let src = "@iri(\"https://e.org/T\")\ntype T\n    \
               @iri(\"https://e.org/a\")\n    @assert(a <= b)\n    a: Integer\n    \
               @iri(\"https://e.org/b\") b: Integer\n";
    let d = the_one(src, |p| matches!(p, Problem::OutOfScope { .. }));
    assert_eq!(
        d.problem,
        Problem::OutOfScope {
            field: "b".to_string(),
            on: "the field `a`".to_string()
        }
    );
    assert!(
        d.help.as_deref().is_some_and(|h| h.contains("type T")),
        "{d:#?}"
    );
}

#[test]
fn an_assertion_on_a_type_reads_its_single_valued_fields() {
    let src = "@iri(\"https://e.org/T\")\n@assert(str.length(tags) > 0)\ntype T\n    \
               @iri(\"https://e.org/tags\") tags: Seq<String>\n";
    assert_eq!(
        the_one(src, |p| matches!(p, Problem::OutOfScope { .. })).problem,
        Problem::OutOfScope {
            field: "tags".to_string(),
            on: "the type `T`".to_string()
        }
    );
}

#[test]
fn a_name_the_type_does_not_have_is_not_a_field_of_it() {
    let all = problems(&one_field("@assert(b > 0)\n    a: Integer"));
    assert!(
        matches!(all.as_slice(), [Problem::UnknownField { field, .. }] if field == "b"),
        "{all:#?}"
    );
}

#[test]
fn a_severity_is_one_of_three() {
    let d = the_one(&one_field("@severity(warnig)\n    a: Seq<String>"), |p| {
        matches!(p, Problem::UnknownSeverity { .. })
    });
    assert_eq!(d.help.as_deref(), Some("did you mean `warning`?"));
}

#[test]
fn a_constant_an_assertion_refuses_is_refused_where_it_is_written() {
    let mapping = |age: &str| {
        format!(
            "{PERSONA}\nPeople : Persona from rows\n    @subject = \"https://e.org/p/{{rows.id}}\"\n    \
             nombre = \"x\"\n    edad = {age}\n    email = rows.mail\n    \
             alta = parse.date(\"2020-01-01\", \"%Y-%m-%d\")\n"
        )
    };
    let d = the_one(&mapping("200"), |p| {
        matches!(p, Problem::AssertFailed { .. })
    });
    assert_eq!(
        d.problem,
        Problem::AssertFailed {
            property: "edad".to_string(),
            value: "200".to_string(),
            assertion: "edad >= 0 and edad < 150".to_string(),
        }
    );
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.labels.len(), 1, "the assertion is labelled: {d:#?}");
    assert_eq!(problems(&mapping("30")), Vec::<Problem>::new());
    assert_eq!(problems(&mapping("rows.age")), Vec::<Problem>::new());
}

#[test]
fn a_warning_assertion_warns() {
    let src = "@iri(\"https://e.org/T\")\ntype T\n    @iri(\"https://e.org/a\")\n    \
               @assert(str.length(a) <= 3, severity = warning)\n    a: String\n\n\
               rows := io.csv(\"u.csv\")\n\n\
               Ts : T from rows\n    @subject = \"https://e.org/t/{rows.id}\"\n    a = \"abcd\"\n";
    let d = the_one(src, |p| matches!(p, Problem::AssertFailed { .. }));
    assert_eq!(d.severity, Severity::Warning);
}
