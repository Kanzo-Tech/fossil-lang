//! The pyramid, addressed — as projections of one sequence, in quarters.
//!
//! `apps/corpus/conformance/expected.json` pins what three readers agree on and
//! is the harness next door. This file is the one that holds the Rust reader
//! against the **writer's own algebra**: every scale below either comes out of
//! `fossil_sinks::manifest::VertexLevels` or is a border the shared table has no
//! case for.
//!
//! **Why the base is never spelled here.** `stride` and `stride_bits` are the
//! one home for the pyramid's exponent, and a test that wrote `4` beside the
//! reader would be the fourteenth spelling — the defect the constant exists to
//! prevent, reintroduced in the file that is supposed to catch it. So the scales
//! a manifest declares are computed through the same algebra the writer calls,
//! and the row counts that are literal (`75`, `19`, `62`) are there precisely
//! because a reader that had kept HALVES would produce a different literal:
//! `150`, `38`, `123`.
//!
//! **The reader itself contains no exponent at all**, which is what the move to
//! `scale` bought: it divides a count by the scale and shifts by its trailing
//! zeros, and `VertexLevels` is imported here only to write the fixtures.

use std::collections::BTreeMap;

use fossil_graph::plan::{Direction, ReadPlan, resolve};
use fossil_sinks::manifest::VertexLevels;

/// A corpus of one type and one self-relation, both declaring the projections
/// the writer would plan for them. `vertex_count` and the level list are the
/// knobs; everything else is the smallest manifest that addresses itself.
fn corpus(vertex_count: Option<u64>, levels: Option<&[u32]>, edge_levels: bool) -> ReadPlan {
    let count = vertex_count.map_or(String::new(), |c| format!("vertex_count: {c}\n"));
    // A level's declared `scale` is `VertexLevels::stride`, never a literal —
    // the writer's algebra is what a fixture has to agree with.
    let block = |list: &[u32], aligned: &str| {
        list.iter().fold(String::new(), |mut acc, &k| {
            use std::fmt::Write as _;
            let _ = write!(
                acc,
                "  - path: l{k}/\n    scale: {}\n{aligned}",
                VertexLevels::stride(k)
            );
            acc
        })
    };
    let vertex_levels = levels.map_or(String::new(), |list| block(list, ""));
    let relation_levels = match (levels, edge_levels) {
        (Some(list), true) => block(list, "    aligned_by: src\n"),
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
            format!(
                "type: Person\nprefix: vertex/Person/\nchunk_size: 64\n{count}\
                 projections:\n  - path: ''\n    scale: 1\n{vertex_levels}"
            ),
        ),
        (
            "edge/knows.edge.yml".to_string(),
            format!(
                "src_type: Person\ndst_type: Person\nedge_type: knows\n\
                 prefix: edge/person_knows_person/\n\
                 chunk_size: 64\nsrc_chunk_size: 64\ndst_chunk_size: 64\n\
                 projections:\n  - path: by_source/\n    scale: 1\n    aligned_by: src\n\
                 {relation_levels}"
            ),
        ),
    ]);
    resolve(&files, "").expect("a corpus that addresses itself")
}

/// `VertexLevels::planned(300, 64)` is levels 1 and 2 — the conformance corpus's
/// own size, and the pyramid the writer emits for it.
const SMALL: &[u32] = &[1, 2];

/// The scale of a level, out of the one place the exponent lives.
const fn scale(level: u32) -> u64 {
    VertexLevels::stride(level)
}

// ── the vertex side: a payload is a projection ───────────────────────────────

/// **The payload is the projection whose scale is one**, and the reader needs no
/// special case to say so: one row per id, no bits dropped, the type's own
/// address. Level 0 used to be a case a level block handled beside the
/// payload; there is no second thing here to keep in step.
#[test]
fn the_payload_is_the_projection_whose_scale_is_one() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let vertex = corpus.vertex_type(None).expect("Person");
    let payload = vertex.projection(1).expect("a payload");

    assert_eq!(payload.rows, vertex.count, "scale 1 drops no row");
    assert_eq!(payload.tiles, vertex.tiles, "scale 1 drops no tile");
    assert_eq!(payload.prefix, vertex.prefix, "and it is at the type's own");
    assert_eq!(payload.column, "dense_id");
    assert_eq!(payload.direction, None);
    for dense_id in [0, 63, 64, 299, 4095, 1 << 31, 1 << 53] {
        assert_eq!(
            payload.tile_of(dense_id),
            vertex.tile_of(dense_id),
            "scale 1 addresses {dense_id} somewhere other than the payload does"
        );
    }
    // And it is one list: the payload and the pyramid, finest first.
    let scales: Vec<u64> = vertex.projections.iter().map(|p| p.scale).collect();
    assert_eq!(scales, vec![1, scale(1), scale(2)]);
}

/// The whole of the change, on the smallest corpus that has a pyramid: a
/// projection drops [`VertexLevels::stride_bits`] bits and not one.
///
/// The literals are the discriminator. At 300 rows in tiles of 64, quarters give
/// scale 4 **75** rows in 2 tiles and scale 16 **19** rows in 1; halves would
/// give 150 in 3 and 75 in 2, and every one of those numbers is a plausible
/// pyramid.
#[test]
fn a_projection_drops_a_quarter_and_not_a_half() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let vertex = corpus.vertex_type(None).expect("Person");
    let one = vertex.projection(scale(1)).expect("the first level");
    let two = vertex.projection(scale(2)).expect("the second");

    assert_eq!(one.rows, Some(75), "ceil(300 / stride(1))");
    assert_eq!(two.rows, Some(19), "ceil(300 / stride(2))");
    assert_eq!(one.tiles, Some(2));
    assert_eq!(two.tiles, Some(1));

    // The same rows, out of the writer's own algebra rather than a literal — and
    // out of the reader's own division, which never sees the exponent.
    for level in SMALL {
        let projection = vertex.projection(scale(*level)).expect("a level");
        assert_eq!(
            projection.rows,
            Some(VertexLevels::rows_at(300, *level)),
            "the reader's row count for level {level} is not the writer's"
        );
        assert_eq!(
            projection.rows,
            vertex.count.map(|c| c.div_ceil(scale(*level)))
        );
    }

    // The address: `chunk_size` is 64, so the payload shifts by 6 and a level by
    // `6 + stride_bits(k)`. 128 is the border that tells the two bases apart —
    // one tile in quarters, tile 1 in halves.
    assert_eq!(vertex.shift, 6);
    assert_eq!(one.shift, 6 + VertexLevels::stride_bits(1));
    assert_eq!(one.tile_of(128), 0, "quarters put 128 in scale 4's tile 0");
    assert_eq!(one.tile_of(255), 0);
    assert_eq!(one.tile_of(256), 1, "64 · stride(1) is where tile 1 starts");
    assert_eq!(two.tile_of(1023), 0);
    assert_eq!(two.tile_of(1024), 1);
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
    let vertex = corpus.vertex_type(None).expect("Person");

    // The declared tile is 64 here, not the plan's 4,096, so the tile counts are
    // this corpus's — the ROWS are the plan's, and those are what the base moves.
    let top = vertex.projection(scale(coarsest)).expect("the coarsest");
    assert_eq!(top.rows, Some(3907), "ceil(1e6 / stride(4))");
    assert_eq!(
        vertex.projection(scale(1)).expect("the first").rows,
        Some(250_000),
        "halves would say 500,000"
    );
    assert_eq!(
        top.tiles,
        Some(VertexLevels::rows_at(1_000_000, coarsest).div_ceil(64))
    );
    // A scale past the coarsest is still a predicate and still shrinks — and it
    // is not written, which is the next test.
    assert!(vertex.projection(scale(coarsest + 1)).is_none());
}

/// A projection the manifest does not declare is not addressable and is still
/// answerable — the refusal has to say which, or a reader composes `l7/` against
/// a corpus that can draw level 7 perfectly well.
#[test]
fn a_scale_nobody_wrote_is_refused_by_name() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let vertex = corpus.vertex_type(None).expect("Person");

    assert!(vertex.projection(scale(1)).is_some() && vertex.projection(scale(2)).is_some());
    assert!(
        vertex.projection(1).is_some(),
        "scale 1 is the payload, and it is in the list rather than beside it"
    );
    assert!(vertex.projection(scale(3)).is_none());

    let message = vertex
        .projection_files(scale(3))
        .expect_err("addressed a scale nobody wrote")
        .to_string();
    assert!(
        message.contains("at scales 1, 4, 16 and not 64"),
        "the refusal does not name the written scales: {message}"
    );
    assert!(
        message.contains("predicate over the payload"),
        "the refusal does not say what answers the scale: {message}"
    );

    assert_eq!(
        vertex.projection_files(scale(1)).expect("written"),
        vec![
            "vertex/Person/l1/chunk0.parquet".to_string(),
            "vertex/Person/l1/chunk1.parquet".to_string(),
        ]
    );
    assert_eq!(
        vertex.projection_files(scale(2)).expect("written"),
        vec!["vertex/Person/l2/chunk0.parquet".to_string()]
    );
    // And the payload enumerates through the same door, because it is the same
    // kind of thing: five tiles of 64 over 300 rows.
    assert_eq!(vertex.files().expect("a payload").len(), 5);
}

// ── the relation side, addressed by the same rule ────────────────────────────

/// A relation's projections are its SOURCE type's, addressed by the rule the
/// vertex projections already have: no second base, no second document, no
/// `edge_count`.
#[test]
fn a_relation_is_addressed_by_its_source_projections() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let vertex = corpus.vertex_type(None).expect("Person");
    let edge = corpus.edges.first().expect("knows");

    // Tile `j` is a range of `src_dense`, so the tile count is the SOURCE's row
    // count over the scale — never `edge_count`, which this corpus does not even
    // declare.
    assert_eq!(
        edge.count, None,
        "no edge_count, and the address does not need one"
    );
    for level in [0, 1, 2] {
        let of_the_relation = edge
            .projection(scale(level), Direction::Src)
            .expect("a relation projection");
        let of_the_type = vertex.projection(scale(level)).expect("a vertex one");
        assert_eq!(of_the_relation.tiles, of_the_type.tiles);
        assert_eq!(of_the_relation.shift, of_the_type.shift);
        assert_eq!(of_the_relation.chunk_size, of_the_type.chunk_size);
        assert_eq!(of_the_relation.column, "src_dense");
        for src_dense in [0u64, 63, 64, 255, 256, 1023, 1024, 1 << 31, 1 << 53] {
            assert_eq!(
                of_the_relation.tile_of(src_dense),
                of_the_type.tile_of(src_dense),
                "scale {} puts src_dense {src_dense} in a tile the vertex projection does not",
                scale(level)
            );
        }
    }

    let two = edge
        .projection(scale(2), Direction::Src)
        .expect("the second level");
    assert_eq!(two.prefix, "edge/person_knows_person/l2/");
    let one = edge
        .projection(scale(1), Direction::Src)
        .expect("the first level");
    assert_eq!(
        one.tile_url(1),
        "edge/person_knows_person/l1/chunk1.parquet"
    );
    assert_eq!(
        edge.projection_files(scale(1), Direction::Src)
            .expect("written"),
        vec![
            "edge/person_knows_person/l1/chunk0.parquet".to_string(),
            "edge/person_knows_person/l1/chunk1.parquet".to_string(),
        ]
    );
    // The adjacency is the same list's scale-1 entry, and it spells its files
    // the way every projection does — `chunk{k}`, not a stem of its own.
    assert_eq!(
        edge.adjacency(Direction::Src).expect("CSR").tile_url(2),
        "edge/person_knows_person/by_source/chunk2.parquet"
    );
}

/// The relation's refusal names the adjacency and the payload, and not the
/// vertex predicate: those are the bytes that answer an unwritten relation
/// level, and a reader handed the wrong sentence debugs the wrong file.
#[test]
fn an_unwritten_relation_scale_names_what_does_answer_it() {
    let corpus = corpus(Some(300), Some(SMALL), true);
    let edge = corpus.edges.first().expect("knows");

    assert!(edge.projection(scale(3), Direction::Src).is_none());
    let message = edge
        .projection_files(scale(3), Direction::Src)
        .expect_err("addressed a scale nobody wrote")
        .to_string();
    assert!(
        message.contains("at scales 1, 4, 16 and not 64"),
        "the refusal does not name the written scales: {message}"
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
    let edge = corpus.edges.first().expect("knows");
    let one = edge
        .projection(scale(1), Direction::Src)
        .expect("the first level");

    assert_eq!(one.tile_of(1024), 4, "the address needs no count");
    assert_eq!(one.tiles, None);
    let message = one
        .files()
        .expect_err("enumerated an unbounded projection")
        .to_string();
    assert!(
        message.contains("addressed by Person, which declares no vertex_count"),
        "the refusal does not name the silent type: {message}"
    );
}

/// A corpus with no level written on either side is a corpus and not a gap: one
/// projection each, which is the payload and the adjacency, and neither reader
/// invents a prefix to 404 against.
#[test]
fn no_pyramid_is_a_corpus_and_not_a_gap() {
    let bare = corpus(Some(300), None, false);
    let vertex = bare.vertex_type(None).expect("Person");
    assert_eq!(vertex.projections.len(), 1);
    assert!(vertex.projection(scale(1)).is_none());
    assert_eq!(bare.edges.first().expect("knows").projections.len(), 1);

    // And a relation may lack the levels its source type has: the vertex pyramid
    // is written, the relation's is not, and that is a byte count rather than a
    // contradiction.
    let half = corpus(Some(300), Some(SMALL), false);
    assert_eq!(half.vertex_type(None).expect("Person").projections.len(), 3);
    assert_eq!(half.edges.first().expect("knows").projections.len(), 1);
}
