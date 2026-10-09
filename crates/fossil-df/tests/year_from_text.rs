//! **An `xsd:gYear` is written from text, as `XPath`'s `xs:gYear` constructor
//! takes it** — `parse.year` — and lands as the year's integer under the
//! `datatype` the shape declared, the column `/docs/format` describes.
//!
//! An integer does not satisfy the slot by itself: `XPath` casts no integer to
//! `xs:gYear`, and the only implicit widening in the language is between two
//! numbers.

#![allow(clippy::literal_string_with_formatting_args)]

#[path = "support/native.rs"]
mod native;
mod support;

const SHAPE: &str = "\
PREFIX ex:  <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Award {
  ex:year xsd:gYear
}
";

/// The Nobel Prize API's spelling: `awardYear` is a string.
const AWARDS: &str = r#"[{"id":"1","awardYear":"1921"},{"id":"2","awardYear":"1945"}]"#;

const PROGRAM: &str = r#"type { Award } := io.shex("award.shex")
Row := io.json("awards.json")

Awards : Award from Row
    @subject = "https://example.org/award/{Row.id}"
    year     = parse.year(Row.awardYear)
"#;

fn messages(program: &str) -> Vec<String> {
    let dir = native::write_dir(&[
        ("award.shex", SHAPE),
        ("awards.json", AWARDS),
        ("award.fossil", program),
    ]);
    native::check(&dir.path().join("award.fossil"))
        .diagnostics
        .iter()
        .map(fossil_base::Diagnostic::message)
        .collect()
}

#[test]
fn parse_year_fills_a_gyear_slot_and_writes_the_year_under_its_datatype() {
    assert!(messages(PROGRAM).is_empty(), "{:#?}", messages(PROGRAM));

    let dir = native::write_dir(&[
        ("award.shex", SHAPE),
        ("awards.json", AWARDS),
        ("award.fossil", PROGRAM),
    ]);
    let corpus = native::run_dir(dir.path(), "award.fossil", &[]).expect("the program runs");
    let award = corpus
        .manifest()
        .vertex_tables
        .iter()
        .find(|v| v.name == "Award")
        .expect("Award")
        .clone();
    let year = award
        .properties
        .iter()
        .find(|p| p.name == "year")
        .expect("year");
    assert_eq!(year.data_type, "int32");
    assert_eq!(
        year.datatype.as_deref(),
        Some("http://www.w3.org/2001/XMLSchema#gYear")
    );

    let root = corpus.materialise();
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let years: Vec<i32> = conn
        .prepare(&format!(
            "SELECT year FROM read_parquet('{}') ORDER BY 1",
            root.path().join(&award.path).display()
        ))
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(years, [1921, 1945]);
}

#[test]
fn an_integer_does_not_fill_a_gyear_slot() {
    let program = PROGRAM.replace("parse.year(Row.awardYear)", "parse.integer(Row.awardYear)");
    assert_eq!(
        messages(&program),
        ["`year` expects GYear, and this is Integer"]
    );
}
