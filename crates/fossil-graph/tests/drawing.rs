//! **What a picture of one vertex type may draw**, and why the incident set is
//! not the answer.
//!
//! A drawing is one type's `dense_id` space. A relation that leaves the type has
//! its far ends numbered in another type's, both spaces are dense from zero, and
//! `BIGINT` compares against `BIGINT` without complaining — so a reader that
//! takes the out-edges of the window as the lines of the picture draws an edge
//! between two vertices with nothing between them, and it looks exactly like a
//! real one.
//!
//! The counterpart on the writing side is `crates/fossil-layout/src/layout/pass.rs`,
//! which computes a placement over the relations whose two ends are both in the
//! type being placed. This file is the reading side of the same sentence, and
//! `crates/fossil-graph/tests/conformance.rs` holds the addresses themselves.

use std::collections::BTreeMap;

use fossil_graph::plan::{Direction, GapReason, ReadPlan, resolve};

/// `Author authored Paper` beside `Author knows Author`: one relation a picture
/// of `Author` can draw and one it cannot, at the same tile size, so the only
/// thing telling them apart is where the far end is numbered.
fn corpus(cross_type_declares_csr: bool) -> ReadPlan {
    let vertex = |name: &str| {
        (
            format!("vertex/{name}.vertex.yml"),
            format!(
                "type: {name}\nprefix: vertex/{name}/\nchunk_size: 64\nvertex_count: 300\n\
                 projections:\n  - path: ''\n    scale: 1\n"
            ),
        )
    };
    let relation = |src: &str, label: &str, dst: &str, csr: bool| {
        let by_source = if csr {
            "  - path: by_source/\n    scale: 1\n    aligned_by: src\n"
        } else {
            ""
        };
        (
            format!("edge/{src}_{label}_{dst}.edge.yml"),
            format!(
                "src_type: {src}\ndst_type: {dst}\nedge_type: {label}\n\
                 prefix: edge/{src}_{label}_{dst}/\n\
                 chunk_size: 64\nsrc_chunk_size: 64\ndst_chunk_size: 64\n\
                 projections:\n{by_source}\
                 \x20 - path: by_target/\n    scale: 1\n    aligned_by: dst\n"
            ),
        )
    };
    let files = BTreeMap::from([
        (
            "graph.graph.yml".to_string(),
            "prefix: ''\nvertices:\n  - vertex/Author.vertex.yml\n  - vertex/Paper.vertex.yml\n\
             edges:\n  - edge/Author_knows_Author.edge.yml\n  - edge/Author_authored_Paper.edge.yml\n"
                .to_string(),
        ),
        vertex("Author"),
        vertex("Paper"),
        relation("Author", "knows", "Author", true),
        relation("Author", "authored", "Paper", cross_type_declares_csr),
    ]);
    resolve(&files, "").expect("a corpus that addresses itself")
}

/// The whole of it: a relation is drawable when both of its endpoints are the
/// drawn type, and `Author authored Paper` is reported rather than dropped.
#[test]
fn a_relation_that_leaves_the_drawn_type_is_not_drawable_and_says_so() {
    let corpus = corpus(true);
    let drawing = corpus.drawing(Some("Author")).expect("Author");

    let drawn: Vec<&str> = drawing
        .relations
        .iter()
        .map(|&i| corpus.edges[i].edge_type.as_str())
        .collect();
    assert_eq!(drawn, vec!["knows"]);
    assert_eq!(drawing.undrawn.len(), 1, "{:?}", drawing.undrawn);
    assert_eq!(drawing.undrawn[0].edge_type, "authored");
    assert_eq!(drawing.undrawn[0].direction, Direction::Src);
    assert_eq!(drawing.undrawn[0].reason, GapReason::OtherSpace);
}

/// The mirror, and the reason the rule is about BOTH endpoints rather than about
/// being the source: a `Paper` is the destination of `authored` and can draw it
/// no more than an `Author` can.
#[test]
fn the_destination_type_cannot_draw_the_relation_either() {
    let corpus = corpus(true);
    let drawing = corpus.drawing(Some("Paper")).expect("Paper");

    assert!(drawing.relations.is_empty());
    assert_eq!(drawing.undrawn.len(), 1);
    assert_eq!(drawing.undrawn[0].edge_type, "authored");
    assert_eq!(drawing.undrawn[0].reason, GapReason::OtherSpace);
}

/// A self-relation the corpus publishes no CSR for is a different fact from one
/// that leaves the type, and it is reported as one.
#[test]
fn a_self_relation_with_no_source_half_is_undrawn_for_the_other_reason() {
    let corpus = corpus(true);
    let files = BTreeMap::from([
        (
            "graph.graph.yml".to_string(),
            "prefix: ''\nvertices:\n  - vertex/Author.vertex.yml\n\
             edges:\n  - edge/Author_knows_Author.edge.yml\n"
                .to_string(),
        ),
        (
            "vertex/Author.vertex.yml".to_string(),
            "type: Author\nprefix: vertex/Author/\nchunk_size: 64\nvertex_count: 300\n\
             projections:\n  - path: ''\n    scale: 1\n"
                .to_string(),
        ),
        (
            "edge/Author_knows_Author.edge.yml".to_string(),
            "src_type: Author\ndst_type: Author\nedge_type: knows\n\
             prefix: edge/Author_knows_Author/\n\
             chunk_size: 64\nsrc_chunk_size: 64\ndst_chunk_size: 64\n\
             projections:\n  - path: by_target/\n    scale: 1\n    aligned_by: dst\n"
                .to_string(),
        ),
    ]);
    let csc_only = resolve(&files, "").expect("a corpus that addresses itself");
    let drawing = csc_only.drawing(None).expect("Author");

    assert!(drawing.relations.is_empty());
    assert_eq!(drawing.undrawn[0].reason, GapReason::NotDeclared);
    // And the reasons are not interchangeable: the same corpus with both halves
    // draws the relation.
    assert_eq!(corpus.drawing(None).expect("Author").relations, vec![0]);
}

/// **The window is unchanged, and it has to be.** `by_source` of
/// `Author authored Paper` holds exactly the out-edges of the windowed authors,
/// so a reader walking a neighbourhood wants it and gets it. What it must not do
/// is come back as a line.
#[test]
fn the_incident_window_still_addresses_the_relation_a_drawing_leaves_out() {
    let corpus = corpus(true);
    let window = corpus
        .window(Some("Author"), &[1], &[Direction::Src, Direction::Dst])
        .expect("a window over Author");

    let addressed: Vec<&str> = window.edges.iter().map(|e| e.edge_type.as_str()).collect();
    assert!(addressed.contains(&"authored"), "{addressed:?}");
    assert_eq!(
        window.edge_urls,
        vec![
            "edge/Author_knows_Author/by_source/chunk1.parquet",
            "edge/Author_knows_Author/by_target/chunk1.parquet",
            "edge/Author_authored_Paper/by_source/chunk1.parquet",
        ]
    );
}
