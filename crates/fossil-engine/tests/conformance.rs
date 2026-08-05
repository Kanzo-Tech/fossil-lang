// The embedded `.fossil` fixture uses template syntax (`${ex:}…/${.id}`) that
// clippy mistakes for format args in a plain string literal — it is not.
#![allow(clippy::literal_string_with_formatting_args)]

//! An artefact validator: what the corpus promises, checked on the corpus.
//!
//! Every assertion here reads the produced tree with **plain SQL over `DuckDB`**
//! and never through `fossil-graph`, `fossil-df` or any of our reader code. That
//! is the entire point. A test that reads back through the writer's own types
//! proves the types are self-consistent and nothing else — it cannot see a
//! promise the format makes to somebody who is not us.
//!
//! ADR-0045 decided this shape on evidence rather than taste — the fourth of the
//! decisions recorded there under «Decidido el 2026-08-05», not its §4, whose
//! heading is about something else. `GraphAr` — whose
//! vocabulary fossil borrowed — ships a shared corpus that every language's CI
//! clones, and **no job writes with one implementation and reads with another**.
//! A fourth implementation landed there having re-derived the path arithmetic
//! differently from the other three *and* from the corpus on disk, with CI green
//! on both sides, because its tests asserted hand-written strings instead of
//! resolving against the artefact. Arrow, by contrast, requires two
//! implementations and integration tests *before* a format change lands, which
//! is why its gaps are enumerated skip-lines instead of wrong answers.
//!
//! **What this cannot prove yet, and it is the larger half.** This is one
//! writer read by one independent engine. It is not the round trip that decision
//! asks for — write with the Rust writer, read with the wasm reader *and* with
//! the TypeScript/DuckDB path, diff the three — because two of those three live
//! in another repository. What is here is the artefact validator that `GraphAr`
//! explicitly lacks; the cross-implementation half arrives with the tile reader.
//!
//! **And it deliberately does not freeze the arithmetic.** No test vector for
//! `morton2` or for chunk addressing appears below, because ADR-0045 put both
//! in motion on the same day: the Morton quantisation may widen if flattened ids
//! are adopted, and the chunk is being retired for a 4,096-row tile. Vectors for
//! an arithmetic that is about to change would be precisely wrong rather than
//! honestly absent. What is asserted here is what survives either decision.

#![cfg(not(target_arch = "wasm32"))]

use std::fmt::Write as _;
use std::path::Path;

use duckdb::Connection;

const PEOPLE: u32 = 300;

const PROGRAM: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"users.csv\")
knows := io.csv(\"knows.csv\")

Person : ex:Person from users
    iri = `${ex:}person/${.id}`
    ex:name = .name

Knows : ex:Person from knows
    iri = `${ex:}person/${.id}`
    ex:name = .name
    // An IRI-valued property *is* an edge: the endpoint resolves against the
    // vertex table by subject IRI, and an inner join drops it if no vertex
    // carries that IRI. It is not projected as a vertex column, which is why
    // the two mappings still union.
    ex:knows = `${ex:}person/${.target}`
";

/// One vertex type, one self-edge, both fed from CSV — the smallest corpus that
/// still exercises the two orderings and more than one chunk boundary would
/// require 122,880 rows, so the chunk-count assertion below is deliberately
/// stated as an invariant over whatever chunks exist rather than as a number.
fn write_fixture(dir: &Path) {
    let mut users = String::from("id,name\n");
    // Both mappings emit `Person`, so both must project the same columns: fossil
    // unions them before dedup, and `check` accepts a mismatch that only `run`
    // rejects. `name` rides along in the edge source for exactly that reason.
    let mut knows = String::from("id,name,target\n");
    for i in 0..PEOPLE {
        let _ = writeln!(users, "{i},person-{i}");
        let _ = writeln!(knows, "{i},person-{i},{}", (i + 1) % PEOPLE);
    }
    std::fs::write(dir.join("users.csv"), users).expect("write users.csv");
    std::fs::write(dir.join("knows.csv"), knows).expect("write knows.csv");
    std::fs::write(dir.join("mapping.fossil"), PROGRAM).expect("write mapping");
}

/// A single scalar out of `DuckDB`, as `i64`. Every check below is a count of
/// violations, so the expected answer is always zero and the failure message can
/// say what was violated rather than what was expected.
fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("query failed: {sql}\n{e}"))
}

#[test]
fn the_corpus_keeps_the_promises_it_makes_to_a_stranger() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());

    let dest = dir.path().join("out");
    // `io.csv` resolves against the process's working directory, so the mapping's
    // bare filenames only mean what they read as from beside them.
    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");
    let status = fossil_engine::run(
        &dir.path().join("mapping.fossil"),
        &format!("file://{}", dest.display()),
        &fossil_engine::RunCreds::default(),
        None,
    );
    std::env::set_current_dir(previous).expect("restore cwd");
    let status = status.expect("fossil run");

    let conn = Connection::open_in_memory().expect("duckdb");
    let vertices = dest.join("vertex/Person/*.parquet");
    let vertices = vertices.display();
    let by_source = dest.join("edge/Person_knows_Person/by_source.parquet");
    let by_source = by_source.display();
    let by_target = dest.join("edge/Person_knows_Person/by_target.parquet");
    let by_target = by_target.display();

    // 0. The corpus is not empty. Every check below is a count of violations, so
    //    an empty file satisfies all of them at once — which is exactly how a
    //    conformance suite comes to pass vacuously and stop being evidence.
    let n = scalar(&conn, &format!("SELECT count(*) FROM '{vertices}'"));
    let edges = scalar(&conn, &format!("SELECT count(*) FROM '{by_source}'"));
    assert_eq!(n, i64::from(PEOPLE), "the corpus lost vertices");
    assert_eq!(edges, i64::from(PEOPLE), "the corpus lost edges");

    // 1. `dense_id` is a gapless 0..n-1. The whole addressing story rests on
    //    this: a tile is a range of dense ids, and a gap means a tile that is
    //    addressable and empty, or a vertex nobody can address.
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(*) FROM '{vertices}' WHERE dense_id < 0 OR dense_id >= {n}")
        ),
        0,
        "dense_id out of 0..{n}"
    );
    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(DISTINCT dense_id) FROM '{vertices}'")
        ),
        n,
        "dense_id repeats"
    );

    // 2. The adjacency files are what their names claim. Measured on the ten
    //    million corpus as zero disorders over 71,024,690 rows; asserted here so
    //    a writer change cannot quietly stop it being true. `by_source` is CSR
    //    and `by_target` is CSC — a reader that trusts the ordering to skip work
    //    gets wrong answers rather than slow ones if this breaks.
    for (file, key) in [(&by_source, "src_dense"), (&by_target, "dst_dense")] {
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT {key} AS k, lag({key}) OVER () AS prev \
                     FROM '{file}') WHERE prev IS NOT NULL AND k < prev"
                )
            ),
            0,
            "{file} is not ordered by {key}"
        );
    }

    // 3. Both orientations carry the same edge set. They are one relation stored
    //    twice, and nothing else in the writer says so.
    assert_eq!(
        scalar(&conn, &format!("SELECT count(*) FROM '{by_source}'")),
        scalar(&conn, &format!("SELECT count(*) FROM '{by_target}'")),
        "the two orientations disagree on how many edges exist"
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM (SELECT src_dense, dst_dense FROM '{by_source}' \
                 EXCEPT SELECT src_dense, dst_dense FROM '{by_target}')"
            )
        ),
        0,
        "an edge exists in by_source and not in by_target"
    );

    // 4. Every endpoint addresses a vertex that exists. A dangling dense id is
    //    the corruption the layout pass's renumbering could introduce and that
    //    no row count would reveal.
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM '{by_source}' e \
                 WHERE NOT EXISTS (SELECT 1 FROM '{vertices}' v WHERE v.dense_id = e.src_dense) \
                    OR NOT EXISTS (SELECT 1 FROM '{vertices}' v WHERE v.dense_id = e.dst_dense)"
            )
        ),
        0,
        "an edge points at a dense_id no vertex has"
    );

    // 5. What the run *told the caller* matches what it wrote. `RunStatus` is
    //    the wire answer a host serves; a corpus that disagrees with its own
    //    status is the failure a consumer cannot detect from either side alone.
    let declared: i64 = status
        .vertices
        .iter()
        .find(|v| v.vertex_type == "Person")
        .expect("Person in RunStatus")
        .count
        .expect("RunStatus carries a vertex count");
    assert_eq!(declared, n, "RunStatus disagrees with the vertex files");

    // 6. The staged single-file vertex Parquet is gone. It is the layout pass's
    //    input, and leaving it behind is a second, stale copy of every vertex —
    //    the kind of thing a reader picks up by globbing and never questions.
    assert!(
        !dest.join("vertex/Person.parquet").exists(),
        "the staged vertex file survived the layout pass"
    );
}
