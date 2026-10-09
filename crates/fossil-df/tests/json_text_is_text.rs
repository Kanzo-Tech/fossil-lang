//! **A JSON string is a string to the checker, as it is to the run.**
//!
//! `read_json_auto` calls `"1845-03-27"` a `DATE`, and the run's reader keeps a
//! JSON string a string. Typed as `DuckDB` guessed, the checker passed
//! `birthday = Row.born` and the run wrote the text under `xsd:date`; it
//! refused `parse.date`, the one spelling that writes a date. `catalogue.bnf`'s
//! json row says which `DESCRIBE` types are text, and both hosts read it.

#![allow(clippy::literal_string_with_formatting_args)]

#[path = "support/native.rs"]
mod native;
mod support;

const SHAPE: &str = "\
PREFIX ex:  <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:birthday xsd:date
}
";

const PEOPLE: &str = r#"[{"id":"1","born":"1845-03-27"},{"id":"2","born":"1853-07-18"}]"#;

fn program(value: &str) -> String {
    format!(
        "type {{ Person }} := io.shex(\"person.shex\")\n\
         Row := io.json(\"people.json\")\n\n\
         People : Person from Row\n    \
             @subject = \"https://example.org/person/{{Row.id}}\"\n    \
             birthday = {value}\n"
    )
}

fn dir(program: &str) -> tempfile::TempDir {
    native::write_dir(&[
        ("person.shex", SHAPE),
        ("people.json", PEOPLE),
        ("person.fossil", program),
    ])
}

fn messages(program: &str) -> Vec<String> {
    native::check(&dir(program).path().join("person.fossil"))
        .diagnostics
        .iter()
        .map(fossil_base::Diagnostic::message)
        .collect()
}

#[test]
fn a_date_shaped_json_string_does_not_fill_a_date_slot() {
    assert_eq!(
        messages(&program("Row.born")),
        ["`birthday` expects Date, and this is String"]
    );
}

#[test]
fn parse_date_over_it_checks_and_writes_a_date() {
    let program = program("parse.date(Row.born, \"%Y-%m-%d\")");
    assert!(messages(&program).is_empty(), "{:#?}", messages(&program));

    let dir = dir(&program);
    let corpus = native::run_dir(dir.path(), "person.fossil", &[]).expect("the program runs");
    let person = corpus
        .manifest()
        .vertex_tables
        .iter()
        .find(|v| v.name == "Person")
        .expect("Person")
        .clone();
    let birthday = person
        .properties
        .iter()
        .find(|p| p.name == "birthday")
        .expect("birthday");
    assert_eq!(birthday.data_type, "date");
}
