// The embedded `.fossil` fixture uses interpolation syntax (`"…{Users.id}"`) that
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
//! The shape was chosen on evidence rather than taste. `GraphAr` — whose
//! vocabulary fossil borrowed — ships a shared corpus that every language's CI
//! clones, and **no job writes with one implementation and reads with another**.
//! A fourth implementation landed there having re-derived the path arithmetic
//! differently from the other three *and* from the corpus on disk, with CI green
//! on both sides, because its tests asserted hand-written strings instead of
//! resolving against the artefact. Arrow, by contrast, requires two
//! implementations and integration tests *before* a format change lands, which
//! is why its gaps are enumerated skip-lines instead of wrong answers.
//!
//! **What this cannot prove, and where the rest of it is now.** This is one
//! writer read by one engine. The cross-implementation half is real and it is
//! not here: `apps/corpus/conformance/expected.json` is a table of addresses
//! executed by three readers that none of them wrote — plain Node
//! (`conformance/reader.mjs`), the published TypeScript (`resolveCorpus`), and
//! `fossil_graph::address`, natively in `crates/fossil-graph/tests/conformance.rs`
//! and as wasm32 through `conformance/wasm-reader.mjs`. This paragraph said two
//! of those three lived in another repository. All three are in this one.
//!
//! What still has one side is the seam between the two: that table resolves
//! against a checked-in corpus, and the corpus THIS file writes is read back by
//! one reader, `apps/corpus/conformance/writer.mjs`. A writer change that moved
//! an address would be caught there, by that reader, and by nothing else — which
//! is a smaller gap than the one this paragraph used to describe and is still a
//! gap. It is named rather than closed because closing it means running the wasm
//! reader over a corpus `fossil run` produced, and that is a Rust test that would
//! need a wasm build to exist.
//!
//! **It does not freeze the Morton arithmetic.** No test vector for `morton2`
//! appears below: the quantisation is in motion — it may widen if flattened
//! ids grow past 32 bits — and a vector for an arithmetic that is
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
type { Person } := io.shex(\"person.shex\")

Users := io.csv(\"users.csv\")
Knows := io.csv(\"knows.csv\")

People : Person from Users
    @subject = \"https://example.org/person/{Users.id}\"
    name = Users.name

Links : Person from Knows
    @subject = \"https://example.org/person/{Knows.id}\"
    name = Knows.name
    // An IRI-valued property *is* an edge: the endpoint resolves against the
    // vertex table by subject IRI, and an inner join drops it if no vertex
    // carries that IRI. It is not projected as a vertex column, which is why
    // the two mappings still union.
    //
    // AND THIS IS THE LINE THE WHOLE TEST IS ABOUT. It used to
    // be `knows = `${ex:}person/${.target}`` — the endpoint IRI written out by
    // hand — and it did not type-check, because the shape declares `knows`
    // with a shape reference as its range, `expected_value_ty` reads that as
    // `Iri`, and an interpolation was `String` outside the identity slot.
    // Nothing in the language produced a per-row `Iri`.
    //
    // `Person(Knows.target)` is that thing: «the Person whose identity is
    // built from this target». The lowering knows the type and uses THE
    // template of that type — the `@subject` above, and a type has exactly
    // one — so the endpoint is not spelled twice and cannot drift from the
    // identity it has to match. That is also what makes the inner join below
    // meaningful rather than lucky.
    knows = Person(Knows.target)
";

/// The output contract the program names, and it is not decoration: a property
/// key is a BARE NAME now, and its meaning is the last segment of
/// a predicate IRI **this document declares** — so without it neither `name`
/// nor `knows` resolves to anything and the program writes no properties at all.
///
/// `knows` is `min: 0` because only the edge mapping writes it; the vertex
/// mapping over `users.csv` satisfies the same shape with `name` alone.
///
/// Its `valueExpr` is a **shape reference** and that is what makes it an edge
/// rather than a sixth vertex column: `apply_output_shape` classifies a
/// predicate as an edge when the SHAPE says its range is a shape (an opaque
/// `nodeKind IRI` or an `xsd:anyURI` is a column). This replaces the template
/// skeleton-matching that used to guess it, which is why the shape document is
/// now load-bearing for the corpus layout and not only for the diagnostics.
const SHAPE: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "https://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "EachOf",
          "expressions": [
            {
              "type": "TripleConstraint",
              "predicate": "https://example.org/name",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            },
            {
              "type": "TripleConstraint",
              "predicate": "https://example.org/knows",
              "valueExpr": "https://example.org/Person",
              "min": 0,
              "max": 1
            }
          ]
        }
      }
    }
  ]
}"#;

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
    std::fs::write(dir.join("person.shex"), SHAPE).expect("write shape document");
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

/// One `key: value` string out of a manifest, by the **whole** line prefix.
///
/// The indent is the discriminator and not decoration: a vertex manifest carries two `prefix` keys,
/// its own at column zero and the index's nested under `index:`. Matching `prefix: ` anywhere would
/// read the first and report that the index is where the tiles are.
fn manifest_line(path: &Path, prefix: &str) -> Option<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read manifest {}: {e}", path.display()));
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|value| value.trim().to_string())
}

/// The count a manifest declares, checked against the rows that are on disk.
///
/// This is where [`fossil_sinks::manifest::VertexInfo::vertex_count`] is
/// answerable for surviving the layout post-pass. The count is computed from the
/// materialised batches BEFORE `enrich_layout` runs, and the pass then re-reads,
/// renumbers, re-tiles and deletes the file it was given — the same order that
/// made `RunStatus.file` name a path that had just stopped existing. It survives
/// because the pass writes one row out for every row it read (it skips a
/// `dense_id` no row carries, never a row), but «it survives because of the
/// shape of the code today» is exactly what this is here to stop being the only
/// evidence. `on_disk` is read back off the tiles the pass wrote, after it ran.
fn declared_count(manifest: &Path, key: &str, on_disk: i64) -> u64 {
    let declared = manifest_number(manifest, key);
    assert_eq!(
        i64::try_from(declared).expect("a row count fits an i64"),
        on_disk,
        "{} declares {key} {declared} and the payload holds {on_disk} — a corpus that lost its \
         tail declares the number it had before the layout pass",
        manifest.display()
    );
    declared
}

/// The footer's box on `key`, one row per row group: the ordinal, how many rows
/// it holds, and the closed range of `key` it covers.
///
/// **This is the reader's index.** A payload set is one Parquet whose row groups
/// are its tiles, so what used to be a directory listing — `tiles_in`, `files_in`
/// and a `chunk{k}.parquet` name to `regexp_extract` an address out of — is this
/// one table, fetched in one range request at the end of the file. Every tiling
/// assertion below is phrased against it.
///
/// **`stats_min_value` and not `stats_min`, and it was measured.** `parquet_metadata`
/// exposes both: `stats_min`/`stats_max` are Parquet's deprecated `min`/`max`
/// thrift fields, and `stats_min_value`/`stats_max_value` are the `min_value`/
/// `max_value` that replaced them because the old pair had no defined ordering
/// for anything but signed integers. `arrow-rs` — which is what writes this
/// corpus — emits only the new pair, so the old one reads back NULL for every
/// row group, every comparison against it is NULL, and every `count` of
/// violations below comes out zero **because there was nothing to compare**.
/// That is why each predicate that uses a bound also fails on a NULL one: this
/// exact shape of vacuity is what the assertions were rewritten out of.
fn boxes(payload: &str, key: &str) -> String {
    format!(
        "(SELECT row_group_id, row_group_num_rows AS rows, \
          stats_min_value::BIGINT AS lo, stats_max_value::BIGINT AS hi \
            FROM parquet_metadata('{payload}') WHERE path_in_schema = '{key}')"
    )
}

/// Every row of a payload set with the tile its own row group addresses —
/// `tile`, from the box's lower bound, and `tile_hi` from its upper.
///
/// The replacement for `read_parquet(glob, filename = true)` plus a regex over
/// the file name, and it has to be the box rather than `row_group_id` because
/// the two only coincide for a fixed-stride set. A vertex tile is exactly
/// `chunk_size` gapless ids, so its ordinal IS its address and the check below
/// asserts that. An adjacency tile is however many edges its vertices happen to
/// have, and a tile whose vertices have none contributes no rows and therefore
/// no row group — the ordinals stay dense while the tile numbers skip, so an
/// ordinal cannot address one and no writer can fix that.
///
/// Rows are attached to their group by `file_row_number` against the running sum
/// of the row counts, which is exact because row groups partition the file in
/// order.
fn addressed(payload: &str, key: &str, shift: u32) -> String {
    format!(
        "(WITH span AS (SELECT *, coalesce(sum(rows) OVER (ORDER BY row_group_id \
             ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS first_row \
           FROM {} b) \
          SELECT r.*, s.row_group_id, s.lo >> {shift} AS tile, s.hi >> {shift} AS tile_hi \
            FROM read_parquet('{payload}', file_row_number = true) r \
            JOIN span s ON r.file_row_number >= s.first_row \
                       AND r.file_row_number < s.first_row + s.rows)",
        boxes(payload, key),
    )
}

// A numbered sequence of assertions over ONE corpus, read top to bottom, and the
// order is the contract rather than an accident of writing. Splitting it either
// threads the same eight bindings through helpers or writes the corpus once per
// helper — the first hides the sequence, the second makes the test minutes long.
#[allow(clippy::cognitive_complexity)]
#[test]
fn the_corpus_keeps_the_promises_it_makes_to_a_stranger() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());

    let dest = dir.path().join("out");
    // No `chdir`. This used to stand the whole process in the fixture directory
    // because `io.csv` resolved against the process's working directory — which
    // is a global, and these tests run on threads, so two of them doing it at
    // once was a race waiting to be written. A source path now resolves against
    // the directory of the program that wrote it, so the fixture means the same
    // thing from anywhere and the test does not have to move to read it.
    introspect(&dir.path().join("mapping.fossil"));
    let report = fossil_cli::run(
        &dir.path().join("mapping.fossil"),
        &format!("file://{}", dest.display()),
        &std::collections::HashMap::new(),
        None,
        None,
    )
    .expect("fossil run");

    // The three payload sets, each ONE file whose row groups are its tiles.
    //
    // They were a `vertex/Person/*.parquet` glob and the two uncut relations
    // `edge/…/by_{source,target}.parquet`. The star was a directory listing,
    // which is the one thing the corpus is designed so nobody performs; the two
    // relations were the writer's staging, published beside their own cut, and
    // `818218c` deletes them. Nothing here reads a path a reader could not
    // compose from the manifest.
    let conn = Connection::open_in_memory().expect("duckdb");
    let vertices = dest.join("vertex/Person/tiles.parquet");
    let vertices = vertices.display();
    let by_source = dest.join("edge/Person_knows_Person/by_source/tiles.parquet");
    let by_source = by_source.display();
    let by_target = dest.join("edge/Person_knows_Person/by_target/tiles.parquet");
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

    // 2. The adjacency payloads are what their prefixes claim, end to end and
    //    not tile by tile — row groups partition the file in order, so one
    //    statement over the whole file is the stronger form of «every tile is
    //    sorted». Measured on the ten million corpus as zero disorders over
    //    71,024,690 rows; asserted here so a writer change cannot quietly stop
    //    it being true. `by_source` is CSR and `by_target` is CSC — a reader
    //    that trusts the ordering to skip work gets wrong answers rather than
    //    slow ones if this breaks.
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

    // 5. What the run *told the caller* matches what it wrote. The report is
    //    the manifest, so this is `vertex_count` a second time — once off the
    //    document on disk (`declared_count` above) and once off stdout. They
    //    come from one call now and cannot disagree; the assertion stays
    //    because that is the property, not the implementation.
    let declared = report
        .vertices
        .iter()
        .find(|v| v.vertex_type == "Person")
        .expect("Person in the report")
        .vertex_count;
    assert_eq!(
        i64::try_from(declared).expect("a row count fits an i64"),
        n,
        "the report disagrees with the vertex files"
    );

    // 6. The staged Parquet the layout pass consumed is gone — the vertex file
    //    AND both adjacencies. Each is the pass's input, and leaving one behind
    //    is a second, stale copy of the rows — the kind of thing a reader picks
    //    up by globbing and never questions. The adjacencies are the newer half:
    //    the pass used to write the remapped relation back over its input and
    //    leave it, so the uncut relation shipped beside its own cut, which is two
    //    containers for one set of rows and one too many.
    for staged in [
        "vertex/Person.parquet",
        "edge/Person_knows_Person/by_source.parquet",
        "edge/Person_knows_Person/by_target.parquet",
    ] {
        assert!(
            !dest.join(staged).exists(),
            "the staged {staged} survived the layout pass"
        );
    }

    // ── the tiling ────────────────────────────────────────────────────────────
    //
    // A tile is a fixed `dense_id` range and its address is a shift, so there is
    // no index to check and nothing to discover: what can go wrong is that a row
    // is not in the tile its id names, and no row count anywhere would show it.
    // Every check below is that one question asked of a different payload set.
    //
    // What moved is where the answer is read. A tile was a file with the address
    // in its name, so the question was a directory listing and a regex; a tile is
    // a row group now, so it is the footer — see [`boxes`] and [`addressed`].

    // 7. The manifest declares the tiling that was emitted. This is the gap the
    //    tiling work is about — the manifest declared a chunking the writer did
    //    not emit for months, and nothing failed, because a promise nobody
    //    checks is free to make. The size is read from the artefact and every later
    //    assertion derives from it, so this test cannot agree with the writer by
    //    sharing a constant with it.
    let tile_rows = manifest_number(&dest.join("vertex/Person.vertex.yml"), "chunk_size");
    assert!(
        tile_rows.is_power_of_two(),
        "the manifest declares a tile of {tile_rows} rows, which no shift addresses"
    );
    let shift = tile_rows.trailing_zeros();

    //    And which container those tiles are in, which is the one thing about a
    //    tile's URL a reader is told rather than derives — listing a directory
    //    is how it would work it out, and there is no listing over HTTP. Every
    //    path above was composed as `<prefix>tiles.parquet`; this is the field
    //    that says a reader may compose it that way, and a corpus that wrote row
    //    groups while declaring `files` sends every reader to `chunk0.parquet`.
    assert_eq!(
        manifest_line(&dest.join("graph.graph.yml"), "container: "),
        Some("rowgroups".to_string()),
        "the graph document declares a container the writer did not use"
    );

    //    And the manifest says how many rows there are, which for a long time it
    //    could not — see [`declared_count`], which is also where that field is
    //    made answerable for surviving the layout post-pass.
    let declared_vertices =
        declared_count(&dest.join("vertex/Person.vertex.yml"), "vertex_count", n);
    declared_count(
        &dest.join("edge/Person_knows_Person/Person_knows_Person.edge.yml"),
        "edge_count",
        edges,
    );

    //    The tile count is now derivable from the manifest alone, which is what
    //    the field buys a reader that cannot list a directory. Derived from the
    //    declaration and not from `PEOPLE`, so a writer that under-declares is
    //    caught by the file check below rather than agreeing with the fixture.
    let expected_tiles = declared_vertices.div_ceil(tile_rows);
    assert!(
        expected_tiles >= 3,
        "non-vacuity: {PEOPLE} rows in tiles of {tile_rows} is {expected_tiles} tile(s), \
         which has no boundary to get wrong"
    );
    let vertex_boxes = boxes(&vertices.to_string(), "dense_id");
    assert_eq!(
        u64::try_from(scalar(
            &conn,
            &format!("SELECT count(*) FROM {vertex_boxes}")
        ))
        .expect("a row-group count fits a u64"),
        expected_tiles,
        "the vertex tiles are not the {expected_tiles} the manifest implies"
    );

    //    And each of them holds exactly the `dense_id` range its ordinal names.
    //    This is the whole of the addressing story for a fixed-stride set: tile
    //    `k` is `[k << shift, (k+1) << shift)`, clipped by the count, so the
    //    ordinal IS the address and there is nothing to look up. It read
    //    `chunk{k}.parquet` and checked that the file existed; a row group
    //    cannot be missing without the ordinals after it shifting down, which is
    //    what this catches instead — and it catches a mistiled range as well,
    //    which the existence check never could.
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM {vertex_boxes} \
                   WHERE lo IS NULL OR hi IS NULL \
                      OR lo <> row_group_id << {shift} \
                      OR hi <> least((row_group_id + 1) << {shift}, {n}) - 1 \
                      OR rows <> hi - lo + 1"
            )
        ),
        0,
        "a vertex tile is not the dense_id range its ordinal names"
    );

    // 7b. The identity index, which the manifest declares and nothing here read.
    //     It is what turns a subject IRI back into an address, so it is the half
    //     of the contract a bookmark, a link from another system and a selection
    //     all depend on — and it is tiled by the same shift, one index tile per
    //     vertex tile, or a reader cannot address it from an id it already holds.
    let index_dir = dest.join("vertex/Person/index");
    assert!(
        index_dir.is_dir(),
        "the manifest declares an identity index and the prefix it names is not there"
    );
    assert_eq!(
        manifest_line(&dest.join("vertex/Person.vertex.yml"), "  prefix: "),
        Some("index/".to_string()),
        "the index prefix the manifest declares is not the one the writer used"
    );
    let index = index_dir.join("tiles.parquet");
    let index = index.display();
    assert_eq!(
        u64::try_from(scalar(
            &conn,
            &format!("SELECT count(DISTINCT row_group_id) FROM parquet_metadata('{index}')")
        ))
        .expect("a row-group count fits a u64"),
        expected_tiles,
        "the index is not tiled like the vertices it addresses, so a lookup cannot name its tile"
    );

    // 8. Every vertex is in the tile its own id names, asked of the rows and not
    //    only of the boxes. `dense_id >> shift` is the entire index — no table,
    //    no listing — so a row in the wrong tile is a vertex a reader will never
    //    fetch and never miss.
    //
    //    Three clauses because the box is now the thing being trusted: the row
    //    is in the tile its id names, the whole group is that one tile, and for
    //    a fixed-stride set the tile is the ordinal. It used to be one clause
    //    against a number pulled out of the file name with a regex.
    //
    //    Non-vacuity first, and it is not ceremony: [`addressed`] attaches every
    //    row to its row group by arithmetic, and a join that matches nothing
    //    returns no rows — at which point a count of violations is zero and says
    //    so about an empty table. Measured while this was being written: the
    //    first draft read `stats_min`, every bound came back NULL, and four
    //    checks passed over nothing.
    let addressed_vertices = addressed(&vertices.to_string(), "dense_id", shift);
    assert_eq!(
        scalar(&conn, &format!("SELECT count(*) FROM {addressed_vertices}")),
        n,
        "the rows could not be attached to their row groups, so every check on them is vacuous"
    );
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM {addressed_vertices} \
                   WHERE tile IS NULL OR tile_hi IS NULL \
                      OR (dense_id >> {shift}) <> tile \
                      OR tile_hi <> tile OR tile <> row_group_id"
            )
        ),
        0,
        "a vertex is in a tile its dense_id does not name"
    );

    // 9. The edges are tiled too — **both orientations**, each by the tile of
    //    the endpoint its file is ordered by. Source-ordered is CSR, measured
    //    against hoisting an edge to the deepest tile holding both endpoints
    //    (2.29× → 15.86× the over-read, 2.5–3.5× the tiles). Target-ordered is
    //    the other half of a hop: the in-edges of a vertex are in the
    //    `by_target` tile its id falls in, and following only the out-edges is a
    //    wrong answer rather than a partial one. Non-vacuity first: an empty
    //    directory satisfies every check after it.
    //
    //    The pair of (payload, addressing column) is walked rather than written
    //    twice, because a check that only ever ran against `by_source` is how
    //    the target half went untiled while every assertion here passed.
    //
    //    **An ordinal cannot address an adjacency tile**, and that is a property
    //    of the data and not of the writer: a tile of 4,096 sources can hold no
    //    edges at all, it contributes no rows, and a row group is not written to
    //    say so. The ordinals stay dense while the tile numbers skip. What
    //    locates a tile here is the footer's box on the key column, so what is
    //    asserted is that the boxes ascend, do not overlap, and each covers
    //    exactly one tile — which together is «the boxes are a lookup table» and
    //    is the strongest statement the container supports.
    let orientations = [
        ("by_source", &by_source, "src_dense", "dst_dense"),
        ("by_target", &by_target, "dst_dense", "src_dense"),
    ];
    for (name, payload, key, other) in orientations {
        let payload = payload.to_string();
        let occupied = scalar(
            &conn,
            &format!("SELECT count(DISTINCT {key} >> {shift}) FROM '{payload}'"),
        );
        assert!(
            occupied >= 3,
            "non-vacuity: {occupied} occupied {name} tile(s)"
        );
        assert_eq!(
            scalar(
                &conn,
                &format!("SELECT count(*) FROM {}", boxes(&payload, key))
            ),
            occupied,
            "the {name} row groups are not the tiles the {key}s occupy — either a tile was \
             split across two, or an empty one was written and every reader pays for a box \
             that describes nothing"
        );
        let addressed_edges = addressed(&payload, key, shift);
        assert_eq!(
            scalar(&conn, &format!("SELECT count(*) FROM {addressed_edges}")),
            edges,
            "the {name} rows could not be attached to their row groups, so every check on \
             them is vacuous"
        );
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM {addressed_edges} \
                       WHERE tile IS NULL OR tile_hi IS NULL \
                          OR ({key} >> {shift}) <> tile OR tile_hi <> tile"
                )
            ),
            0,
            "an edge is in a {name} tile its {key} does not name"
        );

        // 10. The boxes ascend and do not overlap, which is what makes them an
        //     index: a reader looking for one tile's edges takes the box that
        //     contains its address, and if two boxes could contain it the answer
        //     is a scan. Written as a comparison against the previous box's
        //     upper bound rather than as a sort, because a single pair of
        //     overlapping boxes is invisible in a `count(DISTINCT)`.
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT lo, hi, row_group_id, \
                       lag(hi) OVER (ORDER BY row_group_id) AS prev FROM {}) \
                     WHERE lo IS NULL OR hi IS NULL \
                        OR lo > hi \
                        OR (row_group_id > 0 AND (prev IS NULL OR lo <= prev))",
                    boxes(&payload, key)
                )
            ),
            0,
            "the {name} boxes on {key} do not ascend, or two of them overlap"
        );

        // And the payload is ordered the way its manifest claims, secondary key
        // included — a reader that binary-searches a tile for one vertex's rows
        // is trusting the same `ordered: true` the whole relation carries, and a
        // sort that dropped its second column would leave every count above
        // unchanged.
        //
        // Over the whole file and no longer `PARTITION BY filename`: row groups
        // partition it in order, so this is the per-tile statement plus the
        // statement that consecutive tiles do not cross, and it costs one clause
        // fewer than either.
        assert_eq!(
            scalar(
                &conn,
                &format!(
                    "SELECT count(*) FROM (SELECT {key} AS k, {other} AS v, \
                     lag({key}) OVER (ORDER BY file_row_number) AS pk, \
                     lag({other}) OVER (ORDER BY file_row_number) AS pv \
                     FROM read_parquet('{payload}', file_row_number = true)) \
                     WHERE pk IS NOT NULL AND (k, v) < (pk, pv)"
                )
            ),
            0,
            "the {name} payload is not ordered by ({key}, {other})"
        );
    }

    // What 9 and 10 no longer say, and it is deliberate: «the tiles are the whole
    // relation and nothing else» was a comparison against
    // `by_source.parquet` — the uncut relation, published beside its own cut.
    // There is one container per payload set now, so there is nothing to compare
    // against and the comparison would be the file against itself. What carries
    // that weight instead is already above and is stronger: check 0 pins the row
    // count against the fixture rather than against a sibling file,
    // `declared_count` pins it against `edge_count` in the manifest, checks 3 and
    // 4 pin the two orientations against each other and against the vertices,
    // and 13 pins every edge against the identity it was written from.
    let edge_payload = by_source.to_string();

    // 11. And the corpus, read back through its tiles, is the graph that was
    //     asked for. Byte-identity against a previous build is not available —
    //     the layout is deliberately changing — so what replaces it is the input
    //     itself: the fixture is a ring, `i` knows `(i+1) mod n`, and that is
    //     checkable without knowing anything about how it was written.
    //
    //     Joined through `subject` and never through `dense_id`, because the
    //     renumbering is free to move every id and the IRI is the identity
    //     An assertion phrased in dense ids would be an assertion
    //     about the address.
    let ring = format!(
        "FROM read_parquet('{edge_payload}') e \
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
    // 12. A hop is addressable, which is what the target half was tiled for. For
    //     every vertex, the edges the two tiles its `dense_id` names hold *for
    //     it* are exactly the edges the whole relation holds for it — so the
    //     neighbourhood of a vertex is two files a reader computes the URL of,
    //     and never a scan. The alternative was measured in the browser at a
    //     million vertices: `WITH RECURSIVE` over the edge relation had not
    //     returned one hop from one seed after 45 seconds and stalled the
    //     connection behind it.
    //
    //     Phrased as a disagreement rather than as a count so it cannot pass by
    //     both sides being empty: the addressed read is restricted to the tile
    //     the seed's own id names, and the answer it gives is compared against
    //     the same payload read whole.
    //
    //     The restriction used to be a regex over the tile's file name. It is
    //     `e.tile = v.dense_id >> shift` now — the footer's box, resolved by
    //     [`addressed`] — which is the same predicate a reader evaluates, and
    //     the only one available once a tile stops being a file.
    let hop = |payload: &str, key: &str| {
        let window = addressed(payload, key, shift);
        format!(
            "SELECT count(*) FROM ( \
               (SELECT v.dense_id AS seed, e.src_dense, e.dst_dense \
                  FROM read_parquet('{vertices}') v \
                  JOIN {window} e \
                    ON e.{key} = v.dense_id AND e.tile = (v.dense_id >> {shift}) \
                EXCEPT \
                SELECT v.dense_id, e.src_dense, e.dst_dense \
                  FROM read_parquet('{vertices}') v \
                  JOIN read_parquet('{payload}') e ON e.{key} = v.dense_id) \
               UNION ALL \
               (SELECT v.dense_id, e.src_dense, e.dst_dense \
                  FROM read_parquet('{vertices}') v \
                  JOIN read_parquet('{payload}') e ON e.{key} = v.dense_id \
                EXCEPT \
                SELECT v.dense_id, e.src_dense, e.dst_dense \
                  FROM {window} e \
                  JOIN read_parquet('{vertices}') v \
                    ON e.{key} = v.dense_id AND e.tile = (v.dense_id >> {shift})))"
        )
    };
    assert_eq!(
        scalar(&conn, &hop(&edge_payload, "src_dense")),
        0,
        "the out-edges of a vertex are not all in the by_source tile its dense_id names"
    );
    assert_eq!(
        scalar(&conn, &hop(&by_target.to_string(), "dst_dense")),
        0,
        "the in-edges of a vertex are not all in the by_target tile its dense_id names"
    );

    // 13. THE PAYLOAD HALF, and until this section nothing here read a value.
    //     Every check above is about counts, addresses and ordering, so a corpus
    //     that renumbered every vertex and then attached the wrong name to each of
    //     them satisfies all twelve of them and the manifest as well.
    //
    //     What makes it statable is the fixture rather than a recorded build:
    //     person `i` is named `person-i` and knows `(i+1) mod n`, so the whole
    //     graph is one line of arithmetic and the corpus read back through its
    //     tiles is compared against **what was asked for**.
    //
    //     The identity is the join key on purpose. `dense_id` is an address and
    //     the layout pass reassigns it, so an assertion phrased in dense ids can
    //     only say the corpus agrees with itself. `subject` is what a stranger
    //     holds, and it is the thing that has to survive.
    let ordinal = |column: &str| format!("regexp_extract({column}, '([0-9]+)$', 1)::BIGINT");
    let i = ordinal("subject");

    assert_eq!(
        scalar(
            &conn,
            &format!("SELECT count(*) FROM '{vertices}' WHERE name <> 'person-' || {i}::VARCHAR")
        ),
        0,
        "a vertex carries a name that was not built from its own identity — the pass renumbered \
         the rows and a property did not travel with the row it belongs to"
    );

    // The set, not the count. `n` above says ten thousand rows arrived; this says
    // they are the ten thousand that were asked for, which a duplicate and a
    // missing vertex satisfy together.
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM \
                 ((SELECT {i} AS ord FROM '{vertices}' EXCEPT SELECT ord FROM range(0, {PEOPLE}) t(ord)) \
                  UNION ALL \
                  (SELECT ord FROM range(0, {PEOPLE}) t(ord) EXCEPT SELECT {i} AS ord FROM '{vertices}'))"
            )
        ),
        0,
        "the identities on disk are not the {PEOPLE} the mapping was given — one is duplicated, \
         missing, or spelled differently from the template that built it"
    );

    // And the ring, which is the assertion the renumbering can actually break.
    // Both endpoints are addresses; resolving each back to the identity it now
    // names is the only way to ask whether the edge still connects the two
    // vertices it connected before the corpus was reordered.
    let source_ordinal = ordinal("s.subject");
    let target_ordinal = ordinal("d.subject");
    assert_eq!(
        scalar(
            &conn,
            &format!(
                "SELECT count(*) FROM '{by_source}' e \
                   JOIN '{vertices}' s ON s.dense_id = e.src_dense \
                   JOIN '{vertices}' d ON d.dense_id = e.dst_dense \
                  WHERE {target_ordinal} <> ({source_ordinal} + 1) % {PEOPLE}"
            )
        ),
        0,
        "an edge does not connect the two identities it was written from — the endpoints are \
         dense ids, the pass reassigned them, and this is the half of that operation nothing else \
         here can see"
    );

    // What 12 does not prove is that a hop is *cheap*, only that it is correct
    // from two addresses. The cost is a request count over an HTTP origin and it
    // is measured on the reader's side, not here.
}

/// Introspect before compiling — what `fossil-cli` does, and what `check`/`run`
/// stopped doing for themselves. Without it a program's sources have no
/// forward-propagated types, which is a different (and quietly weaker) answer.
fn introspect(path: &std::path::Path) {
    let system = fossil_cli::host_system(path);
    let _ = fossil_introspect::introspect_program(
        &*system,
        path,
        &std::collections::HashMap::new(),
        &fossil_introspect::RunCreds::default(),
    );
}
