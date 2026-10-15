// `{PersonRow.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! **A multi-valued property is a set** on its vertex type, every value in it —
//! through the whole native host, read back with `DuckDB`.
//!
//! A relational source spells `*` as repeated rows, and a vertex merge that
//! kept the first of them would give a person with three nicknames one. The
//! RDF pivot spells it as a list, which no R2RML term map reads. Both are the
//! same column: a `LIST` of the value, `cardinality: set` in the information
//! file.

use graphar_info::{Cardinality, DataType};

#[path = "support/native.rs"]
mod native;
mod support;

const SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name     xsd:string ;
  ex:nickname xsd:string *
}
";

const PEOPLE: &str = "id|name\n1|Ada\n2|Linus\n3|Grace\n";
const NICKNAMES: &str = "id|nickname\n1|Countess\n1|Enchantress\n1|Countess\n2|Penguin\n";

// Two mappings of one type: `People` writes no nickname, `Nicknamed` one row
// per nickname — and a repeated one, which a set keeps once.
const PROGRAM: &str = r#"type { Person } := io.shex("people.shex")

PersonRow := io.csv("person.csv", delimiter = "|")
NickRow   := io.csv("nickname.csv", delimiter = "|")

People : Person from PersonRow
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name

Nicknamed : Person from PersonRow.join(NickRow, on = PersonRow.id == NickRow.id)
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    nickname = NickRow.nickname
"#;

/// Each vertex's nicknames, joined, and `∅` for a vertex that has none.
fn nicknames(ran: &native::Written) -> Vec<(String, String)> {
    let root = ran.materialise();
    native::rows(&format!(
        "SELECT subject, coalesce(array_to_string(list_sort(nickname), ','), '∅') \
         FROM read_parquet({}) ORDER BY 1",
        native::at(root.path(), &ran.chunk("Person")),
    ))
    .into_iter()
    .map(|r| {
        (
            r[0].clone().expect("a subject"),
            r[1].clone().expect("a list"),
        )
    })
    .collect()
}

#[test]
fn every_value_of_a_relational_source_is_kept() {
    let dir = native::write_dir(&[
        ("people.shex", SHEX),
        ("person.csv", PEOPLE),
        ("nickname.csv", NICKNAMES),
        ("people.fossil", PROGRAM),
    ]);
    let ran = native::run_dir(dir.path(), "people.fossil", &[]).expect("the program runs");

    assert_eq!(ran.vertex_count("Person"), 3);
    let person = ran.vertex("Person");
    assert_eq!(
        person.property_groups.len(),
        1,
        "the nickname is in the group of its vertex type"
    );
    let columns: Vec<_> = person.property_groups[0]
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.cardinality))
        .collect();
    assert_eq!(
        columns,
        [
            ("subject", Cardinality::Single),
            ("name", Cardinality::Single),
            ("nickname", Cardinality::Set),
        ],
        "subject first, then the single-valued, then the sets"
    );
    let nickname = &person.property_groups[0].properties[2];
    assert_eq!(nickname.data_type, DataType::String);
    assert!(nickname.is_nullable, "Grace has none");

    // Ada holds two, and the repeated `Countess` once; Linus one; Grace none.
    assert_eq!(
        nicknames(&ran),
        [
            (
                "https://example.org/person/1".to_string(),
                "Countess,Enchantress".to_string()
            ),
            (
                "https://example.org/person/2".to_string(),
                "Penguin".to_string()
            ),
            ("https://example.org/person/3".to_string(), "∅".to_string()),
        ]
    );
    assert_eq!(
        ran.files.keys().filter(|p| p.contains("nickname")).count(),
        1,
        "the group's chunk, and no table of the property's own"
    );
}

const RDF_SHEX: &str = r#"{ "@context": "http://www.w3.org/ns/shex.jsonld", "type": "Schema", "shapes": [
  {"type":"ShapeDecl","id":"https://ex.org/Person","shapeExpr":{"type":"Shape","expression":{"type":"EachOf","expressions":[
     {"type":"TripleConstraint","predicate":"https://ex.org/name","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"}},
     {"type":"TripleConstraint","predicate":"https://ex.org/nickname","valueExpr":{"type":"NodeConstraint","datatype":"http://www.w3.org/2001/XMLSchema#string"},"min":0,"max":-1}
  ]}}}
] }"#;

const RDF_TTL: &str = r#"@prefix ex: <https://ex.org/> .
<https://ex.org/person/1> a ex:Person ; ex:name "Ada" ; ex:nickname "Countess", "Enchantress" .
<https://ex.org/person/2> a ex:Person ; ex:name "Linus" .
"#;

const RDF_PROGRAM: &str = r#"type { Person as PersonShape } := io.shex("people.shex")

{ Person } := io.rdf("people.ttl", schema = io.shex("people.shex"))

People : PersonShape from Person
    @subject = Person.subject
    name     = Person.name
    nickname = Person.nickname
"#;

#[test]
fn an_rdf_list_is_a_row_per_value() {
    let dir = native::write_dir(&[
        ("people.shex", RDF_SHEX),
        ("people.ttl", RDF_TTL),
        ("people.fossil", RDF_PROGRAM),
    ]);
    let ran = native::run_dir(dir.path(), "people.fossil", &[]).expect("the program runs");

    assert_eq!(ran.vertex_count("Person"), 2);
    let person = ran.vertex("Person");
    let nickname = person.property_groups[0]
        .properties
        .iter()
        .find(|p| p.name == "nickname")
        .expect("nickname");
    assert_eq!(
        (nickname.data_type.clone(), nickname.cardinality),
        (DataType::String, Cardinality::Set),
        "a value, in a list"
    );
    assert_eq!(
        nicknames(&ran),
        [
            (
                "https://ex.org/person/1".to_string(),
                "Countess,Enchantress".to_string()
            ),
            ("https://ex.org/person/2".to_string(), "∅".to_string()),
        ]
    );
}
