// The embedded `.fossil` programs carry `"…{users.id}"` interpolation holes —
// LITERAL fossil source, which clippy mistakes for format args in a plain Rust
// string literal. Same allow, same reason, as `host.rs`'s
// `provider_registry.rs`.
#![allow(clippy::literal_string_with_formatting_args)]

//! F5's done-when, through the binary: a source pipeline that compiles, types,
//! and produces the corpus the program describes — not the one the source has.
//!
//! Every layer below this one is already tested where it lives: the HIR shape in
//! `fossil-hir::lower`, the row algebra in `fossil-hir/tests`, the op chain in
//! `fossil-mir::lower`, the executed rows in `fossil-df/tests/pipeline.rs`. What
//! only this test can see is that they are WIRED — a `Filter` in the graph that
//! the emit op does not read still writes every row, and every one of those
//! suites would stay green.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

mod common;

/// The `fossil` binary this test drives — cargo's own path for it.
///
/// **It used to shell out to `cargo build` and then hard-code
/// `<repo>/target/debug/fossil`**, which is a test that can pass against a
/// binary it did not build: with `CARGO_TARGET_DIR` set — which is how this
/// repository's own instructions say to drive the suite — the build lands
/// elsewhere and that path holds whatever was left there last. Measured on
/// 2026-08-13: the file at the hard-coded path was **29 hours old**, older than
/// the parser rewrite, the provider registry, `@rename` and the edge
/// constructor. Everything this file reported that day was about a compiler
/// nobody had edited.
///
/// `CARGO_BIN_EXE_<name>` is cargo's answer: it is set for an integration test
/// and points at the binary of THIS build, which cargo has already built before
/// the test runs. No path to guess, and no `cargo build` spawned from inside a
/// test — the same fix `crates/fossil-lsp/tests/` took.
fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| PathBuf::from(env!("CARGO_BIN_EXE_fossil")))
}

/// The output contract both programs below name. `ex:city` is optional (`?`)
/// because only the join program writes it — a required predicate the filter
/// program never writes is a different failure, and not the one under test.
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
/// join drops but the filter keeps.
fn workdir(name: &str, program: &str) -> PathBuf {
    let tmp = common::unique_workdir("fossil-cli-pipeline", name);
    std::fs::write(tmp.join("person.shex"), PERSON_SHEX).expect("write person.shex");
    std::fs::write(
        tmp.join("users.csv"),
        "id,name,edad,persona_id\n1,Alice,30,1\n2,Bob,12,2\n3,Carol,45,3\n4,Dave,7,4\n5,Eve,18,5\n",
    )
    .expect("write users.csv");
    std::fs::write(
        tmp.join("ciudades.csv"),
        "persona_id,ciudad\n1,Oviedo\n3,Gijon\n9,Aviles\n",
    )
    .expect("write ciudades.csv");
    std::fs::write(tmp.join("p.fossil"), program).expect("write program");
    tmp
}

fn run(workdir: &Path) -> PathBuf {
    let dest = workdir.join("graph");
    let dest_url = format!("file://{}", dest.display());
    let output = Command::new(fossil_binary())
        .args(["run", "p.fossil", "--dest", &dest_url])
        .current_dir(workdir)
        .output()
        .expect("spawn fossil run");
    assert!(
        output.status.success(),
        "fossil run exited {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    dest
}

/// `subject` of every vertex written, sorted.
fn subjects(dest: &Path, vtype: &str) -> Vec<String> {
    let glob = format!("{}/*.parquet", dest.join("vertex").join(vtype).display());
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT subject FROM read_parquet('{}') ORDER BY subject",
            glob.replace('\'', "''")
        ))
        .expect("prepare");
    stmt.query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

/// `where` reaches the corpus: two of five people are under 18 and are not in it.
#[test]
fn a_filtered_pipeline_writes_only_the_rows_that_pass() {
    let wd = workdir(
        "where",
        "\
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"users.csv\")
adultos := users.where(users.edad >= 18)

User : Person from adultos
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
",
    );
    let dest = run(&wd);
    assert_eq!(
        subjects(&dest, "Person"),
        [
            "https://example.org/user/1",
            "https://example.org/user/3",
            "https://example.org/user/5",
        ]
    );
}

/// `join` reaches the corpus, and brings a column with it: `city` reads
/// `ciudades.ciudad`, which is not a column of `users` at all — and the
/// qualified name is what says so, where the old `.ciudad` named a column of an
/// anonymous row and left which side it came from to be inferred. Eve passes the
/// filter and has no city, so the inner join drops her — the outer join that
/// would keep her with a NULL is deliberately not in the first version, because
/// it fabricates NULLs and no output shape can declare a nullable property yet.
#[test]
fn a_joined_pipeline_writes_a_column_its_source_does_not_have() {
    let wd = workdir(
        "join",
        "\
type { Person } := io.shex(\"person.shex\")

users := io.csv(\"users.csv\")
ciudades := io.csv(\"ciudades.csv\")
localizados := users.join(ciudades, on = users.persona_id == ciudades.persona_id).where(users.edad >= 18)

User : Person from localizados
    @subject = \"https://example.org/user/{users.id}\"
    name = users.name
    city = ciudades.ciudad
",
    );
    let dest = run(&wd);
    assert_eq!(
        subjects(&dest, "Person"),
        ["https://example.org/user/1", "https://example.org/user/3",]
    );

    let glob = format!("{}/*.parquet", dest.join("vertex/Person").display());
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let cities: String = conn
        .query_row(
            &format!(
                "SELECT string_agg(city, ',' ORDER BY subject) FROM read_parquet('{}')",
                glob.replace('\'', "''")
            ),
            [],
            |r| r.get(0),
        )
        .expect("the joined column is in the corpus");
    assert_eq!(cities, "Oviedo,Gijon");
}
