//! The pyramid, addressed — vertex levels and relation levels, in quarters.
//!
//! `apps/corpus/conformance/expected.json` pins what three readers agree on and
//! is the harness next door. This file is the one that holds the Rust reader
//! against the **writer's own algebra**: every number below either comes out of
//! `fossil_sinks::manifest::VertexLevels` or is a border the shared table has no
//! case for.
//!
//! **Why the base is never spelled here.** `stride` and `stride_bits` are the
//! one home for the pyramid's exponent, and a test that wrote `4` beside the
//! reader would be the fourteenth spelling — the defect the constant exists to
//! prevent, reintroduced in the file that is supposed to catch it. So the
//! expectations are computed through the same algebra the reader calls, and the
//! ones that are literal (`75`, `19`, `62`) are there precisely because a reader
//! that had kept HALVES would produce a different literal: `150`, `38`, `123`.

use std::collections::BTreeMap;

use fossil_graph::address::{ResolvedCorpus, resolve};
use fossil_sinks::manifest::VertexLevels;

/// A corpus of one type and one self-relation, both declaring the pyramid the
/// writer would plan for them. `vertex_count` and the `levels:` list are the
/// knobs; everything else is the smallest manifest that addresses itself.
fn corpus(vertex_count: Option<u64>, levels: Option<&[u32]>, edge_levels: bool) -> ResolvedCorpus {
    let count = vertex_count.map_or(String::new(), |c| format!("vertex_count: {c}\n"));
    let block = |list: &[u32]| {
        let written = list.iter().fold(String::new(), |mut acc, k| {
            use std::fmt::Write as _;
            let _ = writeln!(acc, "    - {k}");
            acc
        });
        format!("levels:\n  prefix: l\n  chunk_size: 64\n  levels:\n{written}")
    };
    let vertex_levels = levels.map_or(String::new(), block);
    let relation_levels = match (levels, edge_levels) {
        (Some(list), true) => block(list),
        _ => String::new(),
    };

    let files = BTreeMap::from([
        (
            "graph.graph.yml".to_string(),
            "prefix: ''\nvertices:\n  - vertex/Person.vertex.yml\nedges:\n  - edge/knows.edge.yml\n"
                .to_string(),
        ),
        (
            "vertex/Person.vertex.yml".to_string(),
            format!("type: Person\nprefix: vertex/Person/\nchunk_size: 64\n{count}{vertex_levels}"),
        ),
        (
            "edge/knows.edge.yml".to_string(),
            format!(
                "src_type: Person\ndst_type: Person\nedge_type: knows\n\
                 prefix: edge/person_knows_person/\n\
                 chunk_size: 64\nsrc_chunk_size: 64\ndst_chunk_size: 64\n\
                 adj_lists:\n  - aligned_by: src\n    prefix: by_source/\n{relation_levels}"
            ),
        ),
    ]);
    resolve(&files, "").expect("a corpus that addresses itself")
}

/// `VertexLevels::planned(300, 64)` is levels 1 and 2 — the conformance corpus's
/// own size, and the pyramid the writer emits for it.
const SMALL: &[u32] = &[1, 2];

// ── the vertex side, brought to quarters ─────────────────────────────────────

/// Level 0 is the payload, and the reader must say so without a special case:
/// `stride(0)` is one id and `stride_bits(0)` is no bits, so a level-0 address
/// is the payload's own address.
#[test]
fn level_zero_is_the_payload_itself() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let vertex = corpus.vertex_type(None).expect("Person");
    let levels = vertex.levels.as_ref().expect("a pyramid");

    assert_eq!(levels.rows(0), vertex.count, "level 0 drops no row");
    assert_eq!(levels.tiles(0), vertex.tiles, "level 0 drops no tile");
    for dense_id in [0, 63, 64, 299, 4095, 1 << 31, 1 << 53] {
        assert_eq!(
            levels.tile_of(0, dense_id),
            vertex.tile_of(dense_id),
            "level 0 addresses {dense_id} somewhere other than the payload does"
        );
    }
}

/// The whole of the change, on the smallest corpus that has a pyramid: a level
/// drops [`VertexLevels::stride_bits`] bits and not one.
///
/// The literals are the discriminator. At 300 rows in tiles of 64, quarters give
/// level 1 **75** rows in 2 tiles and level 2 **19** rows in 1; halves would give
/// 150 in 3 and 75 in 2, and every one of those numbers is a plausible pyramid.
#[test]
fn a_level_drops_a_quarter_and_not_a_half() {
    let levels = corpus(Some(300), Some(SMALL), true);
    let levels = levels
        .vertex_type(None)
        .expect("Person")
        .levels
        .clone()
        .expect("a pyramid");

    assert_eq!(levels.rows(1), Some(75), "ceil(300 / stride(1))");
    assert_eq!(levels.rows(2), Some(19), "ceil(300 / stride(2))");
    assert_eq!(levels.tiles(1), Some(2));
    assert_eq!(levels.tiles(2), Some(1));

    // The same rows, out of the writer's own algebra rather than a literal.
    for level in SMALL {
        assert_eq!(
            levels.rows(*level),
            Some(VertexLevels::rows_at(300, *level)),
            "the reader's row count for level {level} is not the writer's"
        );
    }

    // The address: `chunk_size` is 64, so the payload shifts by 6 and level `k`
    // by `6 + stride_bits(k)`. 128 is the border that tells the two bases apart
    // — one tile in quarters, tile 1 in halves.
    assert_eq!(levels.shift, 6);
    assert_eq!(
        levels.tile_of(1, 128),
        0,
        "quarters put 128 in level 1 tile 0"
    );
    assert_eq!(levels.tile_of(1, 255), 0);
    assert_eq!(
        levels.tile_of(1, 256),
        1,
        "64 · stride(1) is where tile 1 starts"
    );
    assert_eq!(levels.tile_of(2, 1023), 0);
    assert_eq!(levels.tile_of(2, 1024), 1);
}

/// The coarsest level fits a single tile — that is what makes it the coarsest,
/// and it is the property the writer's search terminates on.
#[test]
fn the_coarsest_level_is_the_one_that_fits_a_tile() {
    // The bench corpus: a million rows at the writer's own tile size.
    let plan = VertexLevels::planned(1_000_000, 4096).expect("over one tile");
    let coarsest = *plan.levels.last().expect("a level");
    assert_eq!(
        plan.levels,
        vec![1, 2, 3, 4],
        "the plan is complete, 1..=coarsest"
    );

    let corpus = corpus(Some(1_000_000), Some(&plan.levels), true);
    let levels = corpus
        .vertex_type(None)
        .expect("Person")
        .levels
        .clone()
        .expect("a pyramid");

    // The declared tile is 64 here, not the plan's 4,096, so the tile counts are
    // this corpus's — the ROWS are the plan's, and those are what the base moves.
    assert_eq!(levels.rows(coarsest), Some(3907), "ceil(1e6 / stride(4))");
    assert_eq!(levels.rows(1), Some(250_000), "halves would say 500,000");
    assert_eq!(
        levels.tiles(coarsest),
        Some(VertexLevels::rows_at(1_000_000, coarsest).div_ceil(64))
    );
    assert!(
        levels.rows(coarsest + 1) < levels.rows(coarsest),
        "a level past the coarsest is still a predicate and still shrinks"
    );
}

/// A level the manifest does not declare is not addressable and is still
/// answerable — the refusal has to say which, or a reader composes `l7/` against
/// a corpus that can draw level 7 perfectly well.
#[test]
fn a_level_nobody_wrote_is_refused_by_name() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let levels = corpus
        .vertex_type(None)
        .expect("Person")
        .levels
        .clone()
        .expect("a pyramid");

    assert!(levels.has(1) && levels.has(2));
    assert!(
        !levels.has(0),
        "level 0 is the payload and is not written twice"
    );
    assert!(!levels.has(3));

    let message = levels
        .files(3)
        .expect_err("addressed a level nobody wrote")
        .to_string();
    assert!(
        message.contains("writes levels 1, 2 and not 3"),
        "the refusal does not name the written levels: {message}"
    );
    assert!(
        message.contains("predicate over the payload"),
        "the refusal does not say what answers the level: {message}"
    );

    assert_eq!(
        levels.files(1).expect("level 1 is written"),
        vec![
            "vertex/Person/l1/chunk0.parquet".to_string(),
            "vertex/Person/l1/chunk1.parquet".to_string(),
        ]
    );
    assert_eq!(
        levels.files(2).expect("level 2 is written"),
        vec!["vertex/Person/l2/chunk0.parquet".to_string()]
    );
}

// ── the relation side, which the Rust reader did not have ────────────────────

/// A relation's levels are its SOURCE type's, addressed by the rule the vertex
/// levels already have: no second base, no second document, no `edge_count`.
#[test]
fn a_relation_is_addressed_by_its_source_levels() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let vertex = corpus.vertex_type(None).expect("Person");
    let vertex_levels = vertex.levels.as_ref().expect("a pyramid");
    let edge = corpus.edges.first().expect("knows");
    let levels = edge.levels.as_ref().expect("a relation pyramid");

    assert_eq!(levels.levels, vertex_levels.levels, "the source type's own");
    assert_eq!(levels.chunk_size, vertex.chunk_size);
    assert_eq!(levels.shift, vertex.shift);

    // Tile `j` is a range of `src_dense`, so the tile count is the SOURCE's row
    // count over the level — never `edge_count`, which this corpus does not even
    // declare.
    assert_eq!(
        edge.count, None,
        "no edge_count, and the address does not need one"
    );
    for level in [0, 1, 2, 3] {
        assert_eq!(
            levels.tiles(level),
            vertex_levels.tiles(level),
            "level {level} spans a different number of src_dense ranges than of vertices"
        );
        for src_dense in [0u64, 63, 64, 255, 256, 1023, 1024, 1 << 31, 1 << 53] {
            assert_eq!(
                levels.tile_of(level, src_dense),
                vertex_levels.tile_of(level, src_dense),
                "level {level} puts src_dense {src_dense} in a tile the vertex level does not"
            );
        }
    }

    assert_eq!(levels.prefix(2), "edge/person_knows_person/l2/");
    assert_eq!(
        levels.tile_url(1, 1),
        "edge/person_knows_person/l1/chunk1.parquet"
    );
    assert_eq!(
        levels.files(1).expect("level 1 is written"),
        vec![
            "edge/person_knows_person/l1/chunk0.parquet".to_string(),
            "edge/person_knows_person/l1/chunk1.parquet".to_string(),
        ]
    );
}

/// The relation's refusal names the adjacency and the payload, and not the
/// vertex predicate: those are the bytes that answer an unwritten relation
/// level, and a reader handed the wrong sentence debugs the wrong file.
#[test]
fn an_unwritten_relation_level_names_what_does_answer_it() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let levels = corpus
        .edges
        .first()
        .expect("knows")
        .levels
        .clone()
        .expect("a relation pyramid");

    assert!(!levels.has(3));
    let message = levels
        .files(3)
        .expect_err("addressed a level nobody wrote")
        .to_string();
    assert!(
        message.contains("writes levels 1, 2 and not 3"),
        "the refusal does not name the written levels: {message}"
    );
    assert!(
        message.contains("adjacency and the payload"),
        "the refusal borrows the vertex sentence: {message}"
    );
}

/// A relation whose source declares no `vertex_count` has addresses and no
/// count, and the two are different failures: `tile_of` still answers, `tiles`
/// says it cannot, and `files` refuses by naming the SOURCE type as the one that
/// is silent — `edge_count` would not have helped.
#[test]
fn a_source_with_no_count_addresses_tiles_and_cannot_enumerate_them() {
    let corpus = corpus(None, Some(SMALL), true);
    let levels = corpus
        .edges
        .first()
        .expect("knows")
        .levels
        .clone()
        .expect("a relation pyramid");

    assert_eq!(levels.tile_of(2, 1024), 1, "the address needs no count");
    assert_eq!(levels.tiles(1), None);
    let message = levels
        .files(1)
        .expect_err("enumerated an unbounded level")
        .to_string();
    assert!(
        message.contains("source type declaring no vertex_count"),
        "the refusal does not name the silent manifest: {message}"
    );
}

/// A corpus with no `levels:` block on either side is a corpus and not a gap.
/// Both readers report `None`, and neither invents a prefix to 404 against.
#[test]
fn no_pyramid_is_a_corpus_and_not_a_gap() {
    let bare = corpus(Some(300), None, false);
    assert!(bare.vertex_type(None).expect("Person").levels.is_none());
    assert!(bare.edges.first().expect("knows").levels.is_none());

    // And a relation may lack the block its source type has: the vertex pyramid
    // is written, the relation's is not, and that is a byte count rather than a
    // contradiction.
    let half = corpus(Some(300), Some(SMALL), false);
    assert!(half.vertex_type(None).expect("Person").levels.is_some());
    assert!(half.edges.first().expect("knows").levels.is_none());
}
