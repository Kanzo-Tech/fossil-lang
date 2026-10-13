//! **Types declared in the program, and imports by name** — what
//! `/docs/design/declared` says the checker owes, one titled test per claim.
//!
//! A declaration lowers to the vocabulary a document decoder produces, and a
//! mapping that targets one is checked exactly as against an imported shape:
//! the first three tests hold that. The rest are the refusals, each by its code.
//!
//! # What it does NOT prove
//!
//! - **That a run writes what the declaration says.** That is the conformance
//!   program `docs/programs/declared`, executed by `fossil-df`'s
//!   `tests/programs.rs` and read back with `DuckDB`.
//! - **That the decoder of a real `ShEx` document names its shapes as the line
//!   decoder here does.** Both write the shape's full IRI and the import reads
//!   its last segment; `fossil-shex`'s own tests hold the IRIs it writes.

use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_base::{Diagnostic, FossilDb, SourceFile};
use fossil_graph_schema::{Occurs, Primitive, Problem, Term};
use fossil_hir::def_map::def_map;
use fossil_hir::lower::lower_to_hir;
use fossil_hir::shapes::{program_shapes, resolve_target_shape};
use fossil_hir::typecheck_mapping;

/// The document every import below reads: a `Place` and a `Tag`, plus a second
/// `Tag` in another vocabulary, so a local name can be shared.
const PLACES: &str = "\
shape https://geo.example/Place
prop https://geo.example/name string 1 1
shape https://geo.example/Tag
shape https://other.example/Tag
";

const COLUMNS: &[(&str, Primitive)] = &[
    ("id", Primitive::Integer),
    ("name", Primitive::String),
    ("nick", Primitive::String),
    ("city", Primitive::Integer),
];

/// The declaration the clean tests share: an `Option`, a `Seq`, a `Ref` to an
/// imported type, and two fields whose IRIs end alike.
const PERSON: &str = "\
type { Place as City } := io.shex(\"places.shex\")

@iri(\"https://schema.org/Person\")
type Person
    @iri(\"https://schema.org/name\")          name: String
    @iri(\"https://shop.example/vocab/name\")  handle: Option<String>
    @iri(\"https://schema.org/knows\")         knows: Seq<Ref<Person>>
    @iri(\"https://schema.org/homeLocation\")  livesIn: Ref<City>

rows := io.csv(\"u.csv\")
";

fn open(src: &str) -> (FossilDb, SourceFile) {
    let (mut db, file) = db_with_document(src, "places.shex", PLACES);
    register_inferred(&mut db, "u.csv", COLUMNS);
    (db, file)
}

/// Every diagnostic the program produces: the parse's, the file's, and each
/// mapping's body and check.
fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let (db, file) = open(src);
    let mut out: Vec<Diagnostic> = fossil_syntax::parse(&db, file).diagnostics(&db).clone();
    out.extend(lower_to_hir(&db, file).diagnostics(&db).iter().cloned());
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

/// The problems, for a test that asserts on the code and its data.
fn problems(src: &str) -> Vec<Problem> {
    diagnostics(src).into_iter().map(|d| d.problem).collect()
}

/// Exactly one problem matching `want`, so a test cannot pass by finding it in
/// a list of twenty.
fn the_one(src: &str, want: impl Fn(&Problem) -> bool) -> Problem {
    let all = problems(src);
    let hits: Vec<&Problem> = all.iter().filter(|p| want(p)).collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one, got {}:\n{all:#?}",
        hits.len()
    );
    hits[0].clone()
}

fn with_person(rest: &str) -> String {
    format!("{PERSON}\n{rest}")
}

const PEOPLE: &str = "\
Places : City from rows
    @subject = \"https://geo.example/place/{rows.city}\"
    name     = rows.name

People : Person from rows
    @subject = \"https://shop.example/person/{rows.id}\"
    name     = rows.name
    handle   = rows.nick
    livesIn  = City(rows.city)
";

/// A lowered field: its name, its predicate, its count, its term, its targets.
type Lowered<'a> = (&'a str, &'a str, Occurs, Option<&'a Term>, &'a [String]);

/// The declaration lowers to the vocabulary a decoder produces: one shape keyed
/// by the IRI written, each field a constraint carrying the field's own NAME
/// beside its predicate, the count from the constructor and the edge to the IRI
/// `Ref<…>` resolved — through the import's `as`, to the shape the document
/// declares.
#[test]
fn a_declared_type_lowers_to_the_shape_vocabulary() {
    let (db, file) = open(&with_person(PEOPLE));
    let shapes = program_shapes(&db, file)
        .expect("the document reads")
        .expect("the program has types");
    let person = shapes
        .lookup("https://schema.org/Person")
        .expect("the declared type, by its IRI");
    let fields: Vec<Lowered<'_>> = person
        .properties
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.predicate.as_str(),
                p.occurs,
                p.term.as_ref(),
                p.targets.as_slice(),
            )
        })
        .collect();
    let string = Term::Literal(Some(Primitive::String.to_xsd_iri().to_string()));
    assert_eq!(
        fields,
        [
            (
                "name",
                "https://schema.org/name",
                Occurs::ONE,
                Some(&string),
                &[][..]
            ),
            (
                "handle",
                "https://shop.example/vocab/name",
                Occurs {
                    min: 0,
                    max: Some(1)
                },
                Some(&string),
                &[][..]
            ),
            (
                "knows",
                "https://schema.org/knows",
                Occurs { min: 0, max: None },
                None,
                &["https://schema.org/Person".to_string()][..]
            ),
            (
                "livesIn",
                "https://schema.org/homeLocation",
                Occurs::ONE,
                None,
                &["https://geo.example/Place".to_string()][..]
            ),
        ]
    );
    assert!(
        shapes.lookup("https://geo.example/Place").is_some(),
        "the imported document's shapes are the program's too"
    );
}

/// A mapping that targets a declared type is checked as against an imported
/// shape: this program writes every required field, so it is clean — and the
/// two fields whose IRIs both end in `name` are two names that collide nowhere.
#[test]
fn a_mapping_targeting_a_declared_type_is_checked_like_an_imported_one() {
    let src = with_person(PEOPLE);
    assert_eq!(
        problems(&src),
        [],
        "a program that writes what it declares is clean"
    );

    let (db, file) = open(&src);
    let people = def_map(&db, file).mappings(&db)[1];
    let shape = resolve_target_shape(&db, people).expect("`Person` resolves");
    assert!(
        shape
            .constraint_for("https://shop.example/vocab/name")
            .is_some()
    );
    let (names, collisions) = shape.short_names();
    assert_eq!(
        names.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["name", "handle", "knows", "livesIn"]
    );
    assert_eq!(
        collisions,
        [],
        "a field's name is its own, not its IRI's last segment"
    );
}

/// And a mapping that gets it wrong is told so in the words an imported shape
/// gets: a required field never written, a key the type does not declare.
#[test]
fn a_declared_type_refuses_what_an_imported_shape_refuses() {
    let src = with_person(
        "People : Person from rows
    @subject = \"https://shop.example/person/{rows.id}\"
    handle   = rows.nick
    nickname = rows.nick
",
    );
    let all = problems(&src);
    assert!(
        all.iter().any(|p| matches!(p,
            Problem::MissingRequiredProperty { property, .. } if property == "name")),
        "{all:#?}"
    );
    assert!(
        all.iter().any(|p| matches!(p,
            Problem::UnknownProperty { property, .. } if property == "nickname")),
        "{all:#?}"
    );
}

/// `syntax/missing-iri` — a declaration and each of its fields write their IRI,
/// and there is no base to fall back to.
#[test]
fn a_declaration_without_its_iri_is_refused() {
    let p = the_one(
        "type Person\n    @iri(\"https://schema.org/name\") name: String\n",
        |p| matches!(p, Problem::MissingIri { .. }),
    );
    assert_eq!(
        p,
        Problem::MissingIri {
            name: "Person".into()
        }
    );
}

/// `name/unknown-type` — a field's type names nothing a field can have, and the
/// refusal lists what it can.
#[test]
fn an_unknown_field_type_is_refused_listing_the_ones_there_are() {
    let src = "@iri(\"https://schema.org/Person\")\ntype Person\n    \
               @iri(\"https://schema.org/name\") name: Strng\n";
    let p = the_one(src, |p| matches!(p, Problem::UnknownType { .. }));
    let Problem::UnknownType { name, types } = p else {
        unreachable!()
    };
    assert_eq!(name, "Strng");
    assert!(types.iter().any(|t| t == "String") && types.iter().any(|t| t == "Ref"));
    assert!(
        diagnostics(src)
            .iter()
            .any(|d| d.help.as_deref() == Some("did you mean `String`?")),
        "and suggests the near one"
    );
}

/// `type/invalid-field-type` — what the vocabulary cannot carry: a count said
/// twice, a reference to a value, a constructor with nothing in it.
#[test]
fn a_field_type_the_vocabulary_cannot_carry_is_refused() {
    for written in [
        "Option<Seq<String>>",
        "Seq<Option<String>>",
        "Ref<String>",
        "Option",
        "String<Date>",
    ] {
        let src = format!(
            "@iri(\"https://schema.org/Person\")\ntype Person\n    \
             @iri(\"https://schema.org/name\") name: {written}\n"
        );
        let p = the_one(&src, |p| matches!(p, Problem::InvalidFieldType { .. }));
        assert!(
            matches!(&p, Problem::InvalidFieldType { written: w, .. } if w == written),
            "{written}: {p:?}"
        );
    }
}

/// `name/duplicate-field` — a field written twice in one type.
#[test]
fn a_field_written_twice_is_refused() {
    let p = the_one(
        "@iri(\"https://schema.org/Person\")\ntype Person\n    \
         @iri(\"https://schema.org/name\") name: String\n    \
         @iri(\"https://shop.example/name\") name: String\n",
        |p| matches!(p, Problem::DuplicateField { .. }),
    );
    assert_eq!(
        p,
        Problem::DuplicateField {
            type_name: "Person".into(),
            field: "name".into()
        }
    );
}

/// `name/duplicate-type` — a type declared twice.
#[test]
fn a_type_declared_twice_is_refused() {
    let decl = "@iri(\"https://schema.org/Person\")\ntype Person\n    \
                @iri(\"https://schema.org/name\") name: String\n";
    let p = the_one(&format!("{decl}{decl}"), |p| {
        matches!(p, Problem::DuplicateType { .. })
    });
    assert_eq!(
        p,
        Problem::DuplicateType {
            name: "Person".into()
        }
    );
}

/// `name/duplicate-type` — and declared and imported under one name, which is
/// the same mistake: one name, two types.
#[test]
fn a_type_declared_and_imported_under_one_name_is_refused() {
    let p = the_one(
        "type { Place as City } := io.shex(\"places.shex\")\n\
         @iri(\"https://schema.org/City\")\ntype City\n    \
         @iri(\"https://schema.org/name\") name: String\n",
        |p| matches!(p, Problem::DuplicateType { .. }),
    );
    assert_eq!(
        p,
        Problem::DuplicateType {
            name: "City".into()
        }
    );
}

/// `name/unknown-shape` — a `Ref<T>` whose `T` is not a type of the program,
/// the code a mapping header naming one gets.
#[test]
fn a_ref_to_a_type_the_program_does_not_have_is_refused() {
    let p = the_one(
        "@iri(\"https://schema.org/Person\")\ntype Person\n    \
         @iri(\"https://schema.org/homeLocation\") livesIn: Ref<Town>\n",
        |p| matches!(p, Problem::UnknownShape { .. }),
    );
    assert_eq!(
        p,
        Problem::UnknownShape {
            shape: "Town".into(),
            declared: vec!["Person".into()]
        }
    );
}

/// `shape/not-in-document` — an import names a local name the document does
/// not declare, and the refusal lists the ones it does.
#[test]
fn an_import_the_document_does_not_declare_is_refused_listing_the_ones_it_does() {
    let p = the_one(
        "type { Town } := io.shex(\"places.shex\")\n\
         rows := io.csv(\"u.csv\")\n\
         Towns : Town from rows\n    \
         @subject = \"https://geo.example/place/{rows.city}\"\n",
        |p| matches!(p, Problem::NotInDocument { .. }),
    );
    assert_eq!(
        p,
        Problem::NotInDocument {
            name: "Town".into(),
            document: "places.shex".into(),
            declared: vec!["Place".into(), "Tag".into()],
        }
    );
}

/// `shape/ambiguous-import` — two shapes of the document share the imported
/// local name, and the refusal names both IRIs.
#[test]
fn two_shapes_sharing_an_imported_local_name_are_refused_naming_both() {
    let p = the_one(
        "type { Tag } := io.shex(\"places.shex\")\n\
         rows := io.csv(\"u.csv\")\n\
         Tags : Tag from rows\n    \
         @subject = \"https://geo.example/tag/{rows.id}\"\n",
        |p| matches!(p, Problem::AmbiguousImport { .. }),
    );
    assert_eq!(
        p,
        Problem::AmbiguousImport {
            name: "Tag".into(),
            document: "places.shex".into(),
            first: "https://geo.example/Tag".into(),
            second: "https://other.example/Tag".into(),
        }
    );
}
