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
//! **It does not freeze the Morton arithmetic.** No test vector for `morton2`
//! appears below: ADR-0045 §8 puts the quantisation in motion — it may widen if
//! flattened ids grow past 32 bits — and a vector for an arithmetic that is
//! about to change would be precisely wrong rather than honestly absent. The
//! *tile* arithmetic is settled and is frozen, but in `fossil-sinks`, beside the
//! shift it describes, and not here: a border vector is a statement about a
//! function, and this file only makes statements about an artefact.

#![cfg(not(target_arch = "wasm32"))]

use std::fmt::Write as _;
use std::path::Path;

use duckdb::Connection;

/// Enough people to cross two tile boundaries.
///
/// It was 300, and the comment beside it said a second chunk would take 122,880
/// rows so the assertions were phrased as invariants over whatever files existed.
/// A tile is 4,096 rows now, so three tiles cost ten thousand rows and a couple
/// of seconds — and every tile assertion below is about a boundary, which one
/// tile does not have. The last tile is deliberately partial (1,808 rows), since
/// a corpus whose vertex count divides the tile size exactly would never exercise
/// the tail.
const PEOPLE: u32 = 10_000;

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
/// exercises the two orderings and three tiles.
///
/// The edges form a **ring**: person `i` knows person `(i+1) mod n`. That is not
/// decoration. It makes the whole graph statable in one line of arithmetic, so
/// the corpus read back through its tiles can be compared against what was asked
/// for rather than against a previous build — which is the check that replaces
/// byte-identity while the corpus layout is deliberately moving.
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

/// One `key: value` number out of a manifest, by line scan.
///
/// Not `serde_yaml_ng`, and emphatically not `fossil_sinks::VertexInfo`: a
/// stranger reads this file with whatever they have, and deserialising it through
/// the struct that wrote it would prove the struct round-trips and nothing about
/// the artefact. The `kanzo-ui` request harness reads the same key with the same
/// one-line scan, which is the second reader this is standing in for.
fn manifest_number(path: &Path, key: &str) -> u64 {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read manifest {}: {e}", path.display()));
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no `{key}` in {}", path.display()))
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("`{key}` in {} is not a number: {e}", path.display()))
}

/// How many files a directory holds — the count the emitter is answerable for.
fn files_in(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read dir {}: {e}", dir.display()))
        .count() as u64
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

    // ── the tiling ────────────────────────────────────────────────────────────
    //
    // A tile is a fixed `dense_id` range and its address is a shift, so there is
    // no index to check and nothing to discover: what can go wrong is that a row
    // is not in the tile its id names, and no row count anywhere would show it.
    // Every check below is that one question asked of a different family of file.

    // 7. The manifest declares the tiling that was emitted. This is the gap
    //    ADR-0041 is about — the manifest declared a chunking the writer did not
    //    emit for months, and nothing failed, because a promise nobody checks is
    //    free to make. The size is read from the artefact and every later
    //    assertion derives from it, so this test cannot agree with the writer by
    //    sharing a constant with it.
    let tile_rows = manifest_number(&dest.join("vertex/Person.vertex.yml"), "chunk_size");
    assert!(
        tile_rows.is_power_of_two(),
        "the manifest declares a tile of {tile_rows} rows, which no shift addresses"
    );
    let shift = tile_rows.trailing_zeros();
    let expected_tiles = u64::from(PEOPLE).div_ceil(tile_rows);
    assert!(
        expected_tiles >= 3,
        "non-vacuity: {PEOPLE} rows in tiles of {tile_rows} is {expected_tiles} tile(s), \
         which has no boundary to get wrong"
    );
    assert_eq!(
        files_in(&dest.join("vertex/Person")),
        expected_tiles,
        "the vertex tiles are not the {expected_tiles} the manifest implies"
    );
    for k in 0..expected_tiles {
        assert!(
            dest.join(format!("vertex/Person/chunk{k}.parquet"))
                .exists(),
            "tile {k} is missing, so the ids it holds are not addressable"
        );
    }

    // 8. Every vertex is in the tile its own id names. `dense_id >> shift` is the
    //    entire index — no table, no listing, no footer — so a row in the wrong
    //    file is a vertex a reader will never fetch and never miss.
    let in_named_tile = |glob: &str, column: &str| {
        format!(
            "SELECT count(*) FROM read_parquet('{glob}', filename = true) \
             WHERE ({column} >> {shift}) \
                <> regexp_extract(filename, '([0-9]+)\\.parquet$', 1)::BIGINT"
        )
    };
    assert_eq!(
        scalar(&conn, &in_named_tile(&vertices.to_string(), "dense_id")),
        0,
        "a vertex is in a tile its dense_id does not name"
    );

    // 9. The edges are tiled too, by their **source's** tile — CSR, which
    //    ADR-0042 §3.2 measured against hoisting an edge to the deepest tile
    //    holding both endpoints (2.29× → 15.86× the over-read, 2.5–3.5× the
    //    tiles). Non-vacuity first: an empty directory satisfies every check
    //    after it.
    let edge_tiles = dest.join("edge/Person_knows_Person/by_source");
    let edge_glob = edge_tiles.join("*.parquet");
    let edge_glob = edge_glob.display();
    let occupied = scalar(
        &conn,
        &format!("SELECT count(DISTINCT src_dense >> {shift}) FROM '{by_source}'"),
    );
    assert!(
        occupied >= 3,
        "non-vacuity: {occupied} occupied edge tile(s)"
    );
    assert_eq!(
        files_in(&edge_tiles) as i64,
        occupied,
        "the edge tiles emitted are not the tiles the sources occupy — either one \
         is missing, or an empty one was written and every reader pays a request \
         to learn it holds nothing"
    );
    assert_eq!(
        scalar(&conn, &in_named_tile(&edge_glob.to_string(), "src_dense")),
        0,
        "an edge is in a tile its src_dense does not name"
    );

    // 10. The tiles are the whole edge relation and nothing else. Splitting a
    //     file is where rows are silently dropped or written twice, and both
    //     survive every check above.
    assert_eq!(
        scalar(&conn, &format!("SELECT count(*) FROM '{edge_glob}'")),
        edges,
        "the edge tiles hold a different number of edges than the file they cut"
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM ( \
                   (SELECT src_dense, dst_dense FROM '{by_source}' \
                    EXCEPT SELECT src_dense, dst_dense FROM '{edge_glob}') \
                   UNION ALL \
                   (SELECT src_dense, dst_dense FROM '{edge_glob}' \
                    EXCEPT SELECT src_dense, dst_dense FROM '{by_source}'))"
            )
        ),
        0,
        "the edge tiles and the file they cut disagree about which edges exist"
    );

    // 11. And the corpus, read back through its tiles, is the graph that was
    //     asked for. Byte-identity against a previous build is not available —
    //     the layout is deliberately changing — so what replaces it is the input
    //     itself: the fixture is a ring, `i` knows `(i+1) mod n`, and that is
    //     checkable without knowing anything about how it was written.
    //
    //     Joined through `subject` and never through `dense_id`, because the
    //     renumbering is free to move every id and the IRI is the identity
    //     (ADR-0045 §8). An assertion phrased in dense ids would be an assertion
    //     about the address.
    let ring = format!(
        "FROM read_parquet('{edge_glob}') e \
         JOIN read_parquet('{vertices}') s ON s.dense_id = e.src_dense \
         JOIN read_parquet('{vertices}') d ON d.dense_id = e.dst_dense"
    );
    assert_eq!(
        scalar(&conn, &format!("SELECT count(*) {ring}")),
        edges,
        "an edge in the tiles has an endpoint no vertex tile holds"
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) {ring} \
                 WHERE regexp_extract(d.subject, '([0-9]+)$', 1)::BIGINT \
                    <> (regexp_extract(s.subject, '([0-9]+)$', 1)::BIGINT + 1) % {PEOPLE}"
            )
        ),
        0,
        "the tiled corpus is not the ring the fixture asked for"
    );

    // What none of 7–11 can prove: that the tile *size* is the right one. That is
    // a measurement over an HTTP origin, it lives in `kanzo-ui/BENCHMARKS.md`, and
    // it is the one thing here a passing test would happily agree with while the
    // reader paid eighteen times the ideal payload — which is what it did.
    //
    // And check 9 is weaker than it reads, measured by inverting the emitter to
    // cut on `dst_dense`: it caught the mutation with **22 violations of 10,000**,
    // because in a ring the two ends of an edge share a tile except at a boundary.
    // A corpus with long-range edges would separate CSR from any other placement
    // by orders of magnitude more; this one separates them by the boundaries.
    //
    // Nor do they see the target-ordered half. `by_target.parquet` is not tiled,
    // on purpose (ADR-0042 §3.6): it answers "an edge with one endpoint off
    // screen", which no measurement here has ever asked for.
}
