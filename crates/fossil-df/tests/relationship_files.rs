// `{PersonRow.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! **A relationship file, as LDBC SNB publishes them**, through the whole native
//! host: checked, run, and read back with `DuckDB`.
//!
//! Three things a many-to-many file needs, and each was missing:
//!
//! - its header is `Person.id|Person.id|creationDate`, names an identifier
//!   cannot hold, so the program writes `Knows."Person.id"`;
//! - it names one column twice, and the executor has to call the second one
//!   what the checker called it — `Person.id_1`, `DuckDB`'s rule;
//! - an edge read from it is written by a SECOND mapping of a type that already
//!   has one, and every edge the two share is one table, not two under one name.

#[path = "support/native.rs"]
mod native;
mod support;

const SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:City {
  ex:name xsd:string
}

ex:Person {
  ex:name     xsd:string ;
  ex:email    xsd:string ? ;
  ex:livesIn  @ex:City ;
  ex:knows    @ex:Person *
}
";

const CITIES: &str = "id|name\nc1|Oviedo\nc2|Gijón\n";
const PEOPLE: &str =
    "id|name|city|email\n1|Ada|c1|ada@x\n2|Linus|c2|linus@x\n3|Grace|c1|grace@x\n4|Alan|c2|\n";
const KNOWS: &str = "Person.id|Person.id|creationDate\n1|2|2010\n1|3|2011\n2|3|2012\n";

const PROGRAM: &str = r#"type { City, Person } := io.shex("snb.shex")

CityRow   := io.csv("city.csv", delimiter = "|")
PersonRow := io.csv("person.csv", delimiter = "|")
KnowsRow  := io.csv("person_knows_person.csv", delimiter = "|")

Cities : City from CityRow
    @subject = "https://example.org/city/{CityRow.id}"
    name     = CityRow.name

People : Person from PersonRow
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    email    = PersonRow.email
    livesIn  = City(PersonRow.city)

Friends : Person from PersonRow.join(KnowsRow, on = PersonRow.id == KnowsRow."Person.id")
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    livesIn  = City(PersonRow.city)
    knows    = Person(KnowsRow."Person.id_1")
"#;

fn dir(program: &str) -> tempfile::TempDir {
    native::write_dir(&[
        ("snb.fossil", program),
        ("snb.shex", SHEX),
        ("city.csv", CITIES),
        ("person.csv", PEOPLE),
        ("person_knows_person.csv", KNOWS),
    ])
}

/// `(src subject, dst subject)` of every row of one edge table, sorted.
fn pairs(root: &std::path::Path, edge: &str, src: &str, dst: &str) -> Vec<(String, String)> {
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let p = |rel: String| root.join(rel).display().to_string().replace('\'', "''");
    let sql = format!(
        "SELECT s.subject, d.subject FROM read_parquet('{e}') e \
         JOIN read_parquet('{s}') s ON s.dense_id = e.src \
         JOIN read_parquet('{d}') d ON d.dense_id = e.dst ORDER BY 1, 2",
        e = p(format!("edge/{edge}.parquet")),
        s = p(format!("vertex/{src}.parquet")),
        d = p(format!("vertex/{dst}.parquet")),
    );
    let mut stmt = conn.prepare(&sql).expect("prepare");
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

#[test]
fn a_quoted_column_reads_a_header_an_identifier_cannot_spell() {
    let dir = dir(PROGRAM);
    let messages: Vec<String> = native::check(&dir.path().join("snb.fossil"))
        .diagnostics
        .iter()
        .map(fossil_base::Diagnostic::message)
        .collect();
    assert!(messages.is_empty(), "{messages:#?}");

    let corpus = native::run_dir(dir.path(), "snb.fossil", &[]).expect("the program runs");
    let root = corpus.materialise();
    let person = |id: u8| format!("https://example.org/person/{id}");
    assert_eq!(
        pairs(root.path(), "Person_knows_Person", "Person", "Person"),
        [
            (person(1), person(2)),
            (person(1), person(3)),
            (person(2), person(3)),
        ],
        "the first `Person.id` is the source and the second, `Person.id_1`, the target",
    );
}

#[test]
fn two_mappings_of_one_type_write_each_shared_edge_once_with_both_contributions() {
    let dir = dir(PROGRAM);
    let corpus = native::run_dir(dir.path(), "snb.fossil", &[]).expect("the program runs");
    let m = corpus.manifest();

    // `People` and `Friends` both write `livesIn`. One table, listed once.
    let names: Vec<&str> = m.edge_tables.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Person_livesIn_City", "Person_knows_Person"]);
    let reported: Vec<&str> = corpus
        .report
        .dropped
        .iter()
        .map(|d| d.table.as_str())
        .collect();
    assert_eq!(reported, names, "the run report names each table once");

    // `People` alone has Alan (4), who knows nobody and is in no `Friends` row:
    // the table is the union of both mappings, not whichever ran last or first.
    let lives_in = m
        .edge_tables
        .iter()
        .find(|e| e.name == "Person_livesIn_City")
        .expect("livesIn");
    assert_eq!(lives_in.record_count, 4);
    let root = corpus.materialise();
    let city = |id: &str| format!("https://example.org/city/{id}");
    let person = |id: u8| format!("https://example.org/person/{id}");
    assert_eq!(
        pairs(root.path(), "Person_livesIn_City", "Person", "City"),
        [
            (person(1), city("c1")),
            (person(2), city("c2")),
            (person(3), city("c1")),
            (person(4), city("c2")),
        ],
    );
}

#[test]
fn an_optional_property_one_mapping_writes_survives_the_merge_with_one_that_does_not() {
    // `People` writes `email` and `Friends` does not. The two contribute rows
    // for the same three subjects, and each vertex keeps the value one of them
    // wrote — never the other mapping's null.
    let dir = dir(PROGRAM);
    let corpus = native::run_dir(dir.path(), "snb.fossil", &[]).expect("the program runs");
    let person = corpus
        .manifest()
        .vertex_tables
        .into_iter()
        .find(|v| v.name == "Person")
        .expect("Person");
    assert_eq!(person.record_count, 4);
    assert!(person.properties.iter().any(|p| p.name == "email"));
    let root = corpus.materialise();
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let path = root
        .path()
        .join("vertex/Person.parquet")
        .display()
        .to_string();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT subject, coalesce(email, '∅') FROM read_parquet('{path}') ORDER BY subject"
        ))
        .expect("prepare");
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let person = |id: u8| format!("https://example.org/person/{id}");
    assert_eq!(
        rows,
        [
            (person(1), "ada@x".to_string()),
            (person(2), "linus@x".to_string()),
            (person(3), "grace@x".to_string()),
            (person(4), "∅".to_string()),
        ],
    );
}

#[test]
fn two_mappings_split_by_a_filter_both_reach_the_edge_they_share() {
    // The `Comment` case: one source split into two mappings by a predicate,
    // each writing the edge the other writes. The halves are disjoint, so a
    // table holding one half is short by exactly the other.
    let program = r#"type { City, Person } := io.shex("snb.shex")

CityRow   := io.csv("city.csv", delimiter = "|")
PersonRow := io.csv("person.csv", delimiter = "|")

Cities : City from CityRow
    @subject = "https://example.org/city/{CityRow.id}"
    name     = CityRow.name

Oviedo : Person from PersonRow.where(PersonRow.city == "c1")
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    livesIn  = City(PersonRow.city)

Elsewhere : Person from PersonRow.where(PersonRow.city != "c1")
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    livesIn  = City(PersonRow.city)
"#;
    let dir = dir(program);
    let corpus = native::run_dir(dir.path(), "snb.fossil", &[]).expect("the program runs");
    let m = corpus.manifest();
    assert_eq!(m.edge_tables.len(), 1, "{:?}", m.edge_tables);
    assert_eq!(m.edge_tables[0].record_count, 4);
    assert_eq!(corpus.report.dropped.len(), 1);
}

#[test]
fn a_dotted_column_written_bare_is_told_to_quote_it() {
    let program = PROGRAM.replace(r#"KnowsRow."Person.id_1""#, "KnowsRow.Person.id_1");
    let outcome = native::check(&dir(&program).path().join("snb.fossil"));
    assert!(
        outcome.diagnostics.iter().any(|d| d.problem
            == fossil_graph_schema::Problem::DottedColumn {
                reference: "KnowsRow.Person.id_1".into(),
                quoted: r#"KnowsRow."Person.id_1""#.into(),
            }),
        "{:#?}",
        outcome.diagnostics
    );
}

#[test]
fn a_misspelled_quoted_column_is_corrected_in_the_quoted_spelling() {
    let program = PROGRAM.replace(r#"KnowsRow."Person.id_1""#, r#"KnowsRow."Person.id_2""#);
    let outcome = native::check(&dir(&program).path().join("snb.fossil"));
    let refusal = outcome
        .diagnostics
        .iter()
        .find(|d| d.message().contains("is not a field of `KnowsRow`"))
        .unwrap_or_else(|| panic!("{:#?}", outcome.diagnostics));
    assert!(
        refusal.message().starts_with(r#"`"Person.id_2"`"#),
        "{}",
        refusal.message()
    );
    assert_eq!(
        refusal.help.as_deref(),
        Some(r#"did you mean `"Person.id_1"`?"#)
    );
}

#[test]
fn a_hole_reads_a_quoted_column_so_each_row_of_a_relationship_file_is_a_vertex() {
    // `knows` with the date the two met, as a vertex per row of the file: its
    // subject is the two ids, which only the quoted spelling can name, written
    // inside the string's holes (PEP 701's rule — a hole holds an expression).
    // Last in the document, because the binding below names its shapes in
    // the document's order.
    let shex = format!(
        "{SHEX}\nex:Knowing {{\n  ex:since  xsd:integer ;\n  ex:knower @ex:Person ;\n  ex:known  @ex:Person\n}}\n"
    );
    let program = r#"type { City, Person, Knowing } := io.shex("snb.shex")

CityRow   := io.csv("city.csv", delimiter = "|")
PersonRow := io.csv("person.csv", delimiter = "|")
KnowsRow  := io.csv("person_knows_person.csv", delimiter = "|")

Cities : City from CityRow
    @subject = "https://example.org/city/{CityRow.id}"
    name     = CityRow.name

People : Person from PersonRow
    @subject = "https://example.org/person/{PersonRow.id}"
    name     = PersonRow.name
    livesIn  = City(PersonRow.city)

Knowings : Knowing from KnowsRow
    @subject = "https://example.org/knowing/{KnowsRow."Person.id"}-{KnowsRow."Person.id_1"}"
    since    = KnowsRow.creationDate
    knower   = Person(KnowsRow."Person.id")
    known    = Person(KnowsRow."Person.id_1")
"#;
    let dir = native::write_dir(&[
        ("snb.fossil", program),
        ("snb.shex", &shex),
        ("city.csv", CITIES),
        ("person.csv", PEOPLE),
        ("person_knows_person.csv", KNOWS),
    ]);
    let messages: Vec<String> = native::check(&dir.path().join("snb.fossil"))
        .diagnostics
        .iter()
        .map(fossil_base::Diagnostic::message)
        .collect();
    assert!(messages.is_empty(), "{messages:#?}");

    let corpus = native::run_dir(dir.path(), "snb.fossil", &[]).expect("the program runs");
    let root = corpus.materialise();
    let knowing = |a: u8, b: u8| format!("https://example.org/knowing/{a}-{b}");
    let person = |id: u8| format!("https://example.org/person/{id}");
    assert_eq!(
        pairs(root.path(), "Knowing_known_Person", "Knowing", "Person"),
        [
            (knowing(1, 2), person(2)),
            (knowing(1, 3), person(3)),
            (knowing(2, 3), person(3)),
        ],
    );
}
