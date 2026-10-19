// `{PersonRow.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! **A single-valued property holds one value, or the run is refused** —
//! `write/too-many-values`, through the whole native host.
//!
//! Nothing in a program is multi-valued until the rows are: a mapping over a
//! join repeats its subject once per matching row. The vertex merge took the
//! first value it met, so a `nickname ?` offered two kept one of them by the
//! order the engine read the rows in, and the run reported nothing.
//! `multi_valued.rs` is the other half: the same rows into a `*` property keep
//! every value.

#[path = "support/native.rs"]
mod native;
mod support;

const PEOPLE: &str = "id|name|place\n1|Ada|1\n2|Linus|2\n3|Grace|1\n";

const SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name     xsd:string ;
  ex:nickname xsd:string ?
}
";

const PROGRAM: &str = r#"type { Person } := io.shex("people.shex")

PersonRow := io.csv("person.csv", delimiter = "|")
NickRow   := io.csv("nickname.csv", delimiter = "|")

Nicknamed : Person from PersonRow.join(NickRow, on = PersonRow.id == NickRow.id)
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    nickname = NickRow.nickname
"#;

fn run(nicknames: &str) -> Result<native::Written, String> {
    let dir = native::write_dir(&[
        ("people.shex", SHEX),
        ("person.csv", PEOPLE),
        ("nickname.csv", nicknames),
        ("people.fossil", PROGRAM),
    ]);
    native::run_dir(dir.path(), "people.fossil", &[])
}

/// Ada is offered `Countess` and `Enchantress` for a property that holds at
/// most one, and the run names the property, the subject and the count.
#[test]
fn two_values_for_a_single_valued_property_refuse_the_run() {
    let refused = run("id|nickname\n1|Countess\n1|Enchantress\n2|Penguin\n")
        .expect_err("two nicknames for a `?` property must refuse");
    for part in [
        "`Person.nickname` is single-valued",
        "`https://example.org/person/1` has both `Countess` and `Enchantress`",
    ] {
        assert!(refused.contains(part), "missing {part:?} in {refused}");
    }
}

/// The SAME value twice is one value: a join that repeats a row is not a
/// subject with two nicknames, and `name`, repeated on every joined row, is
/// the case every join over a key has.
#[test]
fn one_value_offered_twice_is_one_value() {
    let ran = run("id|nickname\n1|Countess\n1|Countess\n2|Penguin\n").expect("the program runs");
    assert_eq!(ran.vertex_count("Person"), 2);
}

const PLACES: &str = "id|name\n1|London\n2|Helsinki\n";
const LIVES: &str = "person|place\n1|1\n1|2\n2|2\n";

const EDGE_SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Place {
  ex:name xsd:string
}

ex:Person {
  ex:name        xsd:string ;
  ex:isLocatedIn @ex:Place
}
";

const EDGE_PROGRAM: &str = r#"type { Person, Place } := io.shex("people.shex")

PersonRow := io.csv("person.csv", delimiter = "|")
PlaceRow  := io.csv("place.csv", delimiter = "|")
LivesRow  := io.csv("lives.csv", delimiter = "|")

Places : Place from PlaceRow
    @subject = "https://example.org/place/{PlaceRow.id}"
    name     = PlaceRow.name

People : Person from PersonRow.join(LivesRow, on = PersonRow.id == LivesRow.person)
    @subject    = "https://example.org/person/{PersonRow.id}"
    name        = PersonRow.name
    isLocatedIn = "https://example.org/place/{LivesRow.place}"
"#;

/// The relation's half of the rule: Ada is located in London and in Helsinki,
/// and `isLocatedIn` is bare in the shape — exactly one.
#[test]
fn two_destinations_for_a_single_valued_relation_refuse_the_run() {
    let dir = native::write_dir(&[
        ("people.shex", EDGE_SHEX),
        ("person.csv", PEOPLE),
        ("place.csv", PLACES),
        ("lives.csv", LIVES),
        ("people.fossil", EDGE_PROGRAM),
    ]);
    let refused = native::run_dir(dir.path(), "people.fossil", &[])
        .expect_err("two places for a bare `@ex:Place` must refuse");
    for part in [
        "`Person_isLocatedIn_Place` is single-valued",
        "`https://example.org/person/1` has both `https://example.org/place/1` and `https://example.org/place/2`",
    ] {
        assert!(refused.contains(part), "missing {part:?} in {refused}");
    }
}
