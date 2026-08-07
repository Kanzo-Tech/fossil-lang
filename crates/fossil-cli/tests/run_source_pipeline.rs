// The embedded `.fossil` fixture uses template syntax (`${ex:}…/${.id}`) that
// clippy mistakes for format args in a plain string literal — it is not.
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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "fossil-cli", "--bin", "fossil"])
            .status()
            .expect("spawn cargo build");
        assert!(status.success(), "cargo build -p fossil-cli failed");
        let bin = repo_root().join("target").join("debug").join("fossil");
        assert!(bin.exists(), "fossil binary missing at {}", bin.display());
        bin
    })
}

/// Five people, three of them adults; three cities, two of which match a person
/// (and one, `9`, matching nobody). So the answers are all different numbers:
/// 5 rows in, 3 after the filter, 2 after the join, and `Eve` is the row the
/// join drops but the filter keeps.
fn workdir(name: &str, program: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("fossil-cli-pipeline-{name}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create workdir");
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
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    rows
}

/// `where` reaches the corpus: two of five people are under 18 and are not in it.
#[test]
fn a_filtered_pipeline_writes_only_the_rows_that_pass() {
    let wd = workdir(
        "where",
        "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")
adultos := users |> where(.edad >= 18)

User : ex:Person from adultos
    iri = `${ex:}user/${.id}`
    ex:name = .name
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

/// `join` reaches the corpus, and brings a column with it: `ex:city` reads
/// `.ciudad`, which is not a column of `users` at all. Eve passes the filter and
/// has no city, so the inner join drops her (ADR-0054 §2 — the outer join that
/// would keep her with a NULL is deliberately not in the first version).
#[test]
fn a_joined_pipeline_writes_a_column_its_source_does_not_have() {
    let wd = workdir(
        "join",
        "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")
ciudades := io.csv(\"ciudades.csv\")
localizados := users |> join(ciudades, on = .persona_id) |> where(.edad >= 18)

User : ex:Person from localizados
    iri = `${ex:}user/${.id}`
    ex:name = .name
    ex:city = .ciudad
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
