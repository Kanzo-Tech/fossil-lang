// `{PersonRow.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! **A multi-valued property is a table of its own**, a row per value, and every
//! value is in it — through the whole native host, read back with `DuckDB`.
//!
//! A relational source spells `*` as repeated rows, and the vertex merge kept
//! the first of them: a person with three nicknames came out with one. The RDF
//! pivot spells it as a list, which no R2RML term map reads. Both are now the
//! same table — `src`, the value — and the vertex row carries neither.

use fossil_sinks::manifest::{Manifest, PropertyTable};

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

fn property<'m>(m: &'m Manifest, name: &str) -> &'m PropertyTable {
    m.property_tables
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no {name} property table: {m:?}"))
}

#[test]
fn every_value_of_a_relational_source_is_kept() {
    let dir = native::write_dir(&[
        ("people.shex", SHEX),
        ("person.csv", PEOPLE),
        ("nickname.csv", NICKNAMES),
        ("people.fossil", PROGRAM),
    ]);
    let corpus = native::run_dir(dir.path(), "people.fossil", &[]).expect("the program runs");
    let m = corpus.manifest();

    let person = &m.vertex_tables[0];
    assert_eq!(person.record_count, 3);
    let columns: Vec<_> = person.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        columns,
        ["dense_id", "subject", "name"],
        "the vertex row carries no nickname"
    );

    let nickname = property(&m, "Person_nickname");
    assert_eq!(nickname.path, "property/Person_nickname.parquet");
    assert_eq!(nickname.source.references, "Person");
    assert_eq!(
        nickname.record_count, 3,
        "three distinct nicknames, the repeat once"
    );
    let value = &nickname.properties[1];
    assert_eq!(value.iri.as_deref(), Some("https://example.org/nickname"));
    assert_eq!(
        value.datatype.as_deref(),
        Some("http://www.w3.org/2001/XMLSchema#string")
    );

    let root = corpus.materialise();
    assert_eq!(
        native::column(root.path(), &nickname.path, "nickname"),
        ["Countess", "Enchantress", "Penguin"]
    );
    // Ada is `dense_id` 0 and Linus 1: subjects in order, and every value is
    // the vertex it belongs to.
    assert_eq!(
        native::column(root.path(), &nickname.path, "src"),
        ["0", "0", "1"]
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

const RDF_PROGRAM: &str = r#"type { PersonShape } := io.shex("people.shex")

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
    let corpus = native::run_dir(dir.path(), "people.fossil", &[]).expect("the program runs");
    let m = corpus.manifest();

    let columns: Vec<_> = m.vertex_tables[0]
        .properties
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(columns, ["dense_id", "subject", "name"]);
    let nickname = property(&m, "Person_nickname");
    assert_eq!(
        nickname.properties[1].data_type, "string",
        "a value, not a list"
    );
    let root = corpus.materialise();
    assert_eq!(
        native::column(root.path(), &nickname.path, "nickname"),
        ["Countess", "Enchantress"]
    );
}

const CLASH_SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:knows        @ex:Person * ;
  ex:knows_Person xsd:string *
}
";

const CLASH_PROGRAM: &str = r#"type { Person } := io.shex("people.shex")

PersonRow := io.csv("person.csv", delimiter = "|")

People : Person from PersonRow
    @subject     = "https://example.org/person/{PersonRow.id}"
    knows        = Person(PersonRow.id)
    knows_Person = PersonRow.name
"#;

/// `Person`'s `knows_Person` and the relation `Person_knows_Person` are one
/// table name, and a reader attaches one view per name: the run is refused
/// before a byte is written rather than leaving one of them unreadable.
#[test]
fn two_tables_under_one_name_are_refused() {
    let dir = native::write_dir(&[
        ("people.shex", CLASH_SHEX),
        ("person.csv", PEOPLE),
        ("people.fossil", CLASH_PROGRAM),
    ]);
    let refused = native::run_dir(dir.path(), "people.fossil", &[]).expect_err("refused");
    assert!(refused.contains("Person_knows_Person"), "{refused}");
}
