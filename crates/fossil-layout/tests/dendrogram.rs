//! **The dendrogram, from outside the crate** — the API `/docs/design/holons`
//! asks for, exercised over a hierarchy Louvain actually found rather than one
//! stated in a unit test.
//!
//! The unit tests beside `Dendrogram` state their levels, which is the right
//! way to test the *choosing*. What they cannot show is that the choosing
//! survives contact with a real dendrogram: that the levels a planted graph
//! produces stack, that the cut's rungs are a subsequence of them, and that
//! every rung contracts by the factor `VertexLevels` declares — reached here
//! through `Cut::declared_branching`, because a second spelling of that
//! exponent is the bug the constant exists to prevent.

use fossil_layout::layout::community::{Cut, Dendrogram};
use fossil_layout::layout::community_hierarchy;

/// A two-tier planted graph: `supers` super-groups of `per_super` cliques of
/// `size` vertices, every pair of cliques inside a super-group joined by
/// `links` edges, and the super-groups joined in a ring by one edge each.
///
/// Two tiers because one is not a hierarchy — a bag of disjoint cliques gives
/// Louvain exactly one level to find. The `links` matter for the same reason:
/// a single edge between cliques of eight is worth less than the modularity
/// penalty of merging them, so the second level never forms and the graph that
/// looks two-tier on paper is one-tier to the algorithm.
fn planted(supers: u32, per_super: u32, size: u32, links: u32) -> (u32, Vec<(u32, u32)>) {
    let cliques = supers * per_super;
    let base = |s: u32, c: u32| (s * per_super + c) * size;
    let mut edges = Vec::new();
    for c in 0..cliques {
        for a in 0..size {
            for b in (a + 1)..size {
                edges.push((c * size + a, c * size + b));
            }
        }
    }
    for s in 0..supers {
        for a in 0..per_super {
            for b in (a + 1)..per_super {
                // Spread over the members rather than all hanging off vertex
                // zero, which would make the link a property of one vertex.
                for j in 0..links {
                    edges.push((base(s, a) + j % size, base(s, b) + (j * 3) % size));
                }
            }
        }
        if s > 0 {
            edges.push((base(s, 0), base(s - 1, 0)));
        }
    }
    (cliques * size, edges)
}

#[test]
fn a_planted_hierarchy_is_adopted_whole_and_cut_into_a_subsequence() {
    let (vertex_count, edges) = planted(8, 4, 8, 4);
    let levels = community_hierarchy(vertex_count, &edges);
    assert!(
        levels.len() >= 2,
        "a two-tier graph has at least two levels"
    );

    let tree = Dendrogram::new(vertex_count, levels.clone()).expect("Louvain's levels stack");
    assert_eq!(
        tree.levels(),
        levels,
        "adopting the levels copies them, nothing more"
    );
    assert_eq!(tree.depth(), levels.len());
    assert_eq!(tree.group_counts().len(), levels.len());

    let cut = tree.cut(4);
    assert!(
        !cut.is_empty(),
        "a 256-vertex graph does not fit a chunk of 4"
    );

    // A cut chooses; it never invents. Its rungs are dendrogram levels, in
    // order, each strictly coarser than the last.
    let mut previous: Option<usize> = None;
    for rung in cut.rungs() {
        assert!(
            rung.level() < tree.depth(),
            "rung {rung:?} names a level the dendrogram does not have",
        );
        assert_eq!(rung.groups(), tree.group_counts()[rung.level()]);
        if let Some(before) = previous {
            assert!(rung.level() > before, "rungs must ascend: {cut:?}");
        }
        previous = Some(rung.level());
    }
}

#[test]
fn every_rung_of_a_real_cut_contracts_by_the_declared_factor() {
    let (vertex_count, edges) = planted(8, 4, 8, 4);
    let cut = Dendrogram::of(vertex_count, &edges).cut(4);

    let mut below = u64::from(vertex_count);
    for rung in cut.rungs() {
        assert!(
            u64::from(rung.groups()) * Cut::declared_branching() <= below,
            "rung {rung:?} contracts {below} by less than {}×",
            Cut::declared_branching(),
        );
        assert!(u64::from(rung.groups()) <= rung.ceiling());
        below = u64::from(rung.groups());
    }
    // The same floor, as the reported ratio rather than as the integer test
    // above — `contractions` is what an instrument prints, and a report that
    // disagrees with the guarantee is worse than no report.
    let floor = f64::from(u32::try_from(Cut::declared_branching()).expect("a small factor"));
    assert!(
        cut.contractions().iter().all(|&r| r >= floor),
        "contractions: {:?}",
        cut.contractions(),
    );
}

#[test]
fn a_rungs_membership_and_parents_agree_with_each_other() {
    let (vertex_count, edges) = planted(8, 4, 8, 4);
    let tree = Dendrogram::of(vertex_count, &edges);
    let cut = tree.cut(4);
    let Some((finest, coarser)) = cut.rungs().first().zip(cut.rungs().get(1)) else {
        panic!("this graph has two rungs: {cut:?}");
    };

    let fine = tree.membership(finest.level()).expect("a rung is a level");
    let coarse = tree.membership(coarser.level()).expect("a rung is a level");
    let parents = tree
        .parents(finest.level(), coarser.level())
        .expect("a rung above a rung has parents");

    assert_eq!(parents.len(), finest.groups() as usize);
    // The obligation `/docs/design/holons` states first: a vertex's holon at the
    // coarser rung IS the parent of its holon at the finer one. Composed two
    // ways, from the vertices and from the groups, and they are the same map.
    for v in 0..vertex_count as usize {
        assert_eq!(
            coarse[v], parents[fine[v] as usize],
            "vertex {v} disagrees about who its parent is",
        );
    }
}

#[test]
fn the_same_graph_gives_the_same_cut_in_a_second_process_worth_of_work() {
    let (vertex_count, edges) = planted(4, 4, 8, 4);
    // Two independent runs from the same input, nothing shared between them —
    // the same standard `community_hierarchy` is held to, applied to the cut.
    let first = Dendrogram::of(vertex_count, &edges);
    let second = Dendrogram::of(vertex_count, &edges);
    assert_eq!(first, second);
    assert_eq!(first.cut(4), second.cut(4));
    for level in 0..first.depth() {
        assert_eq!(first.membership(level), second.membership(level));
    }
}
