//! **Annotations and language-tagged strings** — what `/docs/design/labels`
//! says the checker owes, one titled test per claim.
//!
//! # What it does NOT prove
//!
//! - **That a run writes the language column.** That is the conformance
//!   program `docs/programs/labels`, executed by `fossil-exec`'s
//!   `tests/programs.rs` and read back with `DuckDB`.
//! - **That a shape document's annotations are read.** `fossil_shapes::shex`'s and
//!   `fossil_shapes::shacl`'s own tests hold the two decoders.

use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_base::{FossilDb, SourceFile};
use fossil_graph_schema::{Annotation, Primitive, Problem, RDF_LANG_STRING, RDFS_LABEL, Term};
use fossil_hir::def_map::def_map;
use fossil_hir::lower::lower_to_hir;
use fossil_hir::shapes::program_shapes;
use fossil_hir::typecheck_mapping;

const COLUMNS: &[(&str, Primitive)] = &[
    ("id", Primitive::Integer),
    ("name", Primitive::STRING),
    ("lang", Primitive::STRING),
];

/// The declaration from the page, shrunk: annotations stacked above the type
/// and a field, and a multilingual name.
const LAUREATE: &str = "\
@iri(\"https://schema.org/Person\")
@annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"Nobel laureate\"@en)
type Laureate
    @iri(\"https://schema.org/name\")
    name: Seq<String<Lang>>

    @iri(\"https://schema.org/alternateName\")
    @annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"alias\"@en)
    @annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"alias\")
    alias: Option<String>

rows := io.csv(\"u.csv\")
";

fn open(src: &str) -> (FossilDb, SourceFile) {
    let (mut db, file) = db_with_document(src, "unused.shex", "");
    register_inferred(&mut db, "u.csv", COLUMNS);
    (db, file)
}

fn problems(src: &str) -> Vec<Problem> {
    let (db, file) = open(src);
    let mut out: Vec<Problem> = fossil_syntax::parse(&db, file)
        .diagnostics(&db)
        .iter()
        .map(|d| d.problem.clone())
        .collect();
    out.extend(
        lower_to_hir(&db, file)
            .diagnostics(&db)
            .iter()
            .map(|d| d.problem.clone()),
    );
    for m in def_map(&db, file).mappings(&db) {
        out.extend(
            fossil_hir::body::body(&db, *m)
                .diagnostics(&db)
                .iter()
                .map(|d| d.problem.clone()),
        );
        out.extend(
            typecheck_mapping(&db, *m)
                .diagnostics(&db)
                .iter()
                .map(|d| d.problem.clone()),
        );
    }
    out
}

fn mapping(value: &str) -> String {
    format!(
        "{LAUREATE}
Laureates : Laureate from rows
    @subject = \"https://nobel.example/laureate/{{rows.id}}\"
    name     = {value}
"
    )
}

/// `@annotation` lowers to `ShEx`'s Annotation on the shape and on the field, in
/// written order, the object a plain or a tagged string; a `String<Lang>` field
/// is a literal of `rdf:langString`.
#[test]
fn an_annotation_lowers_onto_the_type_and_the_field_it_is_written_above() {
    let (db, file) = open(&mapping("rows.name.lang(rows.lang)"));
    let shapes = program_shapes(&db, file)
        .expect("reads")
        .expect("has types");
    let laureate = shapes
        .lookup("https://schema.org/Person")
        .expect("the type");
    assert_eq!(
        laureate.annotations,
        [Annotation::literal(
            RDFS_LABEL,
            "Nobel laureate",
            Some("en")
        )]
    );
    let name = &laureate.properties[0];
    assert_eq!(
        name.term,
        Some(Term::Literal(Some(fossil_graph_schema::Xsd::new(
            RDF_LANG_STRING
        ))))
    );
    assert!(name.annotations.is_empty());
    let alias = &laureate.properties[1];
    assert_eq!(
        alias.annotations,
        [
            Annotation::literal(RDFS_LABEL, "alias", Some("en")),
            Annotation::literal(RDFS_LABEL, "alias", None),
        ]
    );
}

/// A `String<Lang>` field takes `str.lang` over a column with a column tag —
/// `rml:languageMap` — and the program is clean.
#[test]
fn a_per_row_tag_is_str_lang_over_a_column() {
    assert_eq!(problems(&mapping("rows.name.lang(rows.lang)")), []);
    assert_eq!(problems(&mapping("rows.name.lang(\"en\")")), []);
}

/// A constant is the literal, and the literal is a `String<Lang>`.
#[test]
fn a_tagged_literal_is_a_constant_lang_string() {
    assert_eq!(problems(&mapping("\"Marie Curie\"@en")), []);
}

/// A `String` where a `String<Lang>` belongs is the property mismatch it always
/// should have been.
#[test]
fn a_string_is_not_a_lang_string() {
    let all = problems(&mapping("rows.name"));
    assert!(
        all.iter()
            .any(|p| matches!(p, Problem::PropertyMismatch { property, .. } if property == "name")),
        "{all:#?}"
    );
}

/// `type/needless-lang` — `"x".lang("en")` is the literal, written twice.
#[test]
fn a_constant_string_given_a_constant_tag_is_refused_naming_the_literal() {
    let all = problems(&mapping("\"Marie Curie\".lang(\"en\")"));
    assert!(
        all.contains(&Problem::NeedlessLang {
            call: "\"Marie Curie\".lang(\"en\")".to_string(),
            literal: "\"Marie Curie\"@en".to_string(),
        }),
        "{all:#?}"
    );
}

/// `syntax/invalid-language-tag` — a constant tag is held to BCP 47 wherever it
/// is written: an annotation's object, a literal, a constant argument.
#[test]
fn an_ill_formed_constant_tag_is_refused_wherever_it_is_written() {
    let bad = |p: &Problem| matches!(p, Problem::InvalidLanguageTag { tag } if tag == "abcdefghi");
    let annotation = LAUREATE.replace("\"Nobel laureate\"@en", "\"Nobel laureate\"@abcdefghi");
    let src = format!(
        "{annotation}
Laureates : Laureate from rows
    @subject = \"https://nobel.example/laureate/{{rows.id}}\"
    name     = rows.name.lang(rows.lang)
"
    );
    assert_eq!(problems(&src).iter().filter(|p| bad(p)).count(), 1);
    assert_eq!(
        problems(&mapping("\"x\"@abcdefghi"))
            .iter()
            .filter(|p| bad(p))
            .count(),
        1
    );
    assert_eq!(
        problems(&mapping("rows.name.lang(\"abcdefghi\")"))
            .iter()
            .filter(|p| bad(p))
            .count(),
        1
    );
}

/// `syntax/prefixed-iri` — there are no prefixes: `"rdfs:label"` is refused,
/// naming the IRI it abbreviates, and so is an `@iri` written the same way.
#[test]
fn a_prefixed_name_is_refused_naming_the_iri_it_abbreviates() {
    let src = mapping("rows.name.lang(rows.lang)").replacen(
        "\"http://www.w3.org/2000/01/rdf-schema#label\"",
        "\"rdfs:label\"",
        1,
    );
    assert!(problems(&src).contains(&Problem::PrefixedIri {
        written: "rdfs:label".to_string(),
        expands_to: RDFS_LABEL.to_string(),
    }));
    let src = mapping("rows.name.lang(rows.lang)").replace(
        "\"https://schema.org/alternateName\"",
        "\"schema:alternateName\"",
    );
    assert!(problems(&src).contains(&Problem::PrefixedIri {
        written: "schema:alternateName".to_string(),
        expands_to: "http://schema.org/alternateName".to_string(),
    }));
}
