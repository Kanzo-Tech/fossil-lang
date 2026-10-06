// The embedded `.fossil` programs carry `"…{users.id}"` interpolation holes —
// LITERAL fossil source, which clippy mistakes for format args.
#![allow(clippy::literal_string_with_formatting_args)]

//! F5's done-when, through the executor: a source pipeline that compiles,
//! types, and produces the corpus the program describes — not the one the
//! source has.
//!
//! Every layer below this one is already tested where it lives: the HIR shape in
//! `fossil-hir::lower`, the row algebra in `fossil-hir/tests`, the op chain in
//! `fossil-mir::lower`, the executed rows in `tests/pipeline.rs`. What only this
//! test can see is that they are WIRED — a `Filter` in the graph that the emit
//! op does not read still writes every row, and every one of those suites would
//! stay green.

#[path = "support/native.rs"]
mod native;
mod support;

/// The output contract both programs below name. `ex:city` is optional (`?`)
/// because only the join program writes it.
const PERSON_SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string ;
  ex:city xsd:string ?
}
";

/// Five people, three of them adults; three cities, two of which match a person
/// (and one, `9`, matching nobody). So the answers are all different numbers:
/// 5 rows in, 3 after the filter, 2 after the join, and `Eve` is the row the
/// join drops but the filter keeps. Answers the corpus, materialised.
fn run(program: &str) -> tempfile::TempDir {
    let dir = native::write_dir(&[
        ("person.shex", PERSON_SHEX),
        (
            "users.csv",
            "id,name,edad,persona_id\n1,Alice,30,1\n2,Bob,12,2\n3,Carol,45,3\n4,Dave,7,4\n5,Eve,18,5\n",
        ),
        (
            "ciudades.csv",
            "persona_id,ciudad\n1,Oviedo\n3,Gijon\n9,Aviles\n",
        ),
        ("p.fossil", program),
    ]);
    native::run_dir(dir.path(), "p.fossil", &[])
        .expect("the program runs")
        .materialise()
}

/// `where` reaches the corpus: two of five people are under 18 and are not in it.
#[test]
fn a_filtered_pipeline_writes_only_the_rows_that_pass() {
    let root = run("\
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"users.csv\")
adultos := users.where(users.edad >= 18)

User : Person from adultos
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
");
    assert_eq!(
        native::column(root.path(), "vertex/Person.parquet", "subject"),
        [
            "https://example.org/user/1",
            "https://example.org/user/3",
            "https://example.org/user/5",
        ]
    );
}

/// `join` reaches the corpus, and brings a column with it: `city` reads
/// `ciudades.ciudad`, which is not a column of `users` at all. Eve passes the
/// filter and has no city, so the inner join drops her.
#[test]
fn a_joined_pipeline_writes_a_column_its_source_does_not_have() {
    let root = run("\
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"users.csv\")
ciudades := io.csv(\"ciudades.csv\")
localizados := users.join(ciudades, on = users.persona_id == ciudades.persona_id).where(users.edad >= 18)

User : Person from localizados
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
    city = ciudades.ciudad
");
    assert_eq!(
        native::column(root.path(), "vertex/Person.parquet", "subject"),
        ["https://example.org/user/1", "https://example.org/user/3"]
    );

    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let cities: String = conn
        .query_row(
            &format!(
                "SELECT string_agg(city, ',' ORDER BY subject) FROM read_parquet('{}')",
                root.path().join("vertex/Person.parquet").display()
            ),
            [],
            |r| r.get(0),
        )
        .expect("the joined column is in the corpus");
    assert_eq!(cities, "Oviedo,Gijon");
}
