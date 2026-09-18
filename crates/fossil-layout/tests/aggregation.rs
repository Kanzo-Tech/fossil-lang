//! Integration: **a cell and the level below it say the same thing.**
//!
//! A cell is a summary row — a group, how many members it has, its edges, and
//! its parent — and `/docs/design/cells` states its whole contract as three
//! obligations against the level underneath: its members are *exactly* the
//! children that name it as parent, its count is the sum of its children's
//! counts, and its edges are the aggregation of its children's edges.
//!
//! **Nothing else in the tree can catch a divergence.** A cell level is
//! well-formed on its own terms whatever it holds: an aggregate that dropped a
//! child, or counted one twice, or summed the edges of the level below but not
//! the ones *between* siblings, is a tree that opens, addresses, draws, and
//! summarises a graph nobody has. There is no exception to throw, and there is
//! no reader that would notice — a reader descending is asking the tree what is
//! down there, which is precisely the question it would be answering wrongly.
//! The only way to catch it is to evaluate the definition against the level
//! below and diff.
//!
//! So this is `levels.rs`'s standard applied one artefact along:
//! `a_level_file_holds_exactly_what_the_level_predicate_selects` compares a
//! written level against the predicate that defines it rather than trusting the
//! writer, and these compare a cell against the aggregation that defines it.
//! The comparison is **symmetric difference in both directions** for the same
//! reason it is there: one direction passes over an aggregate that is a strict
//! subset of the truth, and equality of counts passes over an aggregate that
//! has the right number of the wrong members. Each obligation below is
//! therefore run twice — green against the truth, and **red against a
//! corruption that only that obligation can see**, because a check that has
//! never failed is a check whose failure path is a guess.
//!
//! **There is no cell writer yet**, so what stands in for one is a `Cell`
//! whose fields are independent the way a written row's are: the count is a
//! field and not `members.len()`, which is the whole of what "not what the
//! writer recorded separately" means. Evaluated over a planted two-tier
//! partition, whose answer is known outside this file by construction, and over
//! a real Louvain cut, whose answer nobody designed.
//!
//! Everything here is `BTreeMap`/`BTreeSet` and index walks. No hash iteration
//! reaches an assertion, because determinism is the property the page puts
//! first and a verification that is itself unstable proves nothing about it.

use std::collections::{BTreeMap, BTreeSet};

use fossil_layout::layout::community::{Cut, Dendrogram};
use fossil_sinks::manifest::{CoordinateSystem, Provenance};

/// The planted graph, and the tier sizes the ground truth below is written in
/// terms of: 8 super-groups of 4 cliques of 8 vertices — 256 vertices, 1,095
/// edges — which is `tests/dendrogram.rs`'s fixture at its own parameters.
const SUPERS: u32 = 8;
const PER_SUPER: u32 = 4;
const SIZE: u32 = 8;
const LINKS: u32 = 4;

/// A two-tier planted graph: `SUPERS` super-groups of `PER_SUPER` cliques of
/// `SIZE` vertices, every pair of cliques inside a super-group joined by
/// `LINKS` edges, and the super-groups joined in a path by one edge each.
///
/// The same construction `tests/dendrogram.rs` plants, and for the same reason:
/// one tier is not a hierarchy, and a single edge between cliques is worth less
/// than the modularity penalty of merging them, so the second tier never forms.
/// Here it buys something that file does not need — **the answer is known
/// before the code runs**. Clique `c` is vertices `c * SIZE ..`, super-group `s`
/// is cliques `s * PER_SUPER ..`, and every edge is one of three kinds, so what
/// each cell must hold is arithmetic rather than a measurement.
fn planted() -> (u32, Vec<(u32, u32)>) {
    let cliques = SUPERS * PER_SUPER;
    let base = |s: u32, c: u32| (s * PER_SUPER + c) * SIZE;
    let mut edges = Vec::new();
    for c in 0..cliques {
        for a in 0..SIZE {
            for b in (a + 1)..SIZE {
                edges.push((c * SIZE + a, c * SIZE + b));
            }
        }
    }
    for s in 0..SUPERS {
        for a in 0..PER_SUPER {
            for b in (a + 1)..PER_SUPER {
                for j in 0..LINKS {
                    edges.push((base(s, a) + j % SIZE, base(s, b) + (j * 3) % SIZE));
                }
            }
        }
        if s > 0 {
            edges.push((base(s, 0), base(s - 1, 0)));
        }
    }
    (cliques * SIZE, edges)
}

/// The two-tier partition **as planted**, adopted as a dendrogram: vertex to
/// clique, clique to super-group.
///
/// Not what Louvain found — what the graph was built out of. A test whose
/// fixture and whose subject are both the algorithm can only say the algorithm
/// agrees with itself.
fn stated() -> (Dendrogram, Vec<(u32, u32)>) {
    let (vertex_count, edges) = planted();
    let cliques = SUPERS * PER_SUPER;
    let to_clique: Vec<u32> = (0..vertex_count).map(|v| v / SIZE).collect();
    let to_super: Vec<u32> = (0..cliques).map(|c| c / PER_SUPER).collect();
    let tree = Dendrogram::new(vertex_count, vec![to_clique, to_super])
        .expect("the planted tiers stack by construction");
    (tree, edges)
}

/// **One summary row, with the fields a written one would have.**
///
/// `count` is a field rather than `members.len()` on purpose: obligation 2 is
/// only an obligation because a writer records the count *beside* the members
/// and the two can disagree. Fold it into the set and the check becomes a
/// tautology that no corruption can fail.
///
/// `edges` is keyed by the ordered pair, and a cell carries every edge it is an
/// **endpoint** of — so a cross edge appears in both of its cells, and
/// [`edge_union`] checks that the two agree about what it weighs. `internal` is
/// the weight with both ends inside, which is not a decoration: it is where the
/// edges between a cell's own children go.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Cell {
    members: BTreeSet<u32>,
    count: u64,
    edges: BTreeMap<(u32, u32), u64>,
    internal: u64,
}

/// The cell rows at one level, **straight from the graph** — every vertex
/// placed by `membership`, every edge counted once.
///
/// This is the recorded side of every diff below. It never consults the level
/// underneath, which is what makes it an independent witness of what
/// [`aggregate`] claims.
fn cells_at(membership: &[u32], groups: usize, edges: &[(u32, u32)]) -> Vec<Cell> {
    let mut out = vec![Cell::default(); groups];
    for (v, &g) in (0u32..).zip(membership) {
        let cell = &mut out[g as usize];
        cell.members.insert(v);
        cell.count += 1;
    }
    for &(u, v) in edges {
        let (a, b) = (membership[u as usize], membership[v as usize]);
        if a == b {
            out[a as usize].internal += 1;
        } else {
            let key = (a.min(b), a.max(b));
            *out[a as usize].edges.entry(key).or_default() += 1;
            *out[b as usize].edges.entry(key).or_default() += 1;
        }
    }
    out
}

/// The distinct edges of a level, taken out of the rows that carry them.
///
/// Both endpoints record the same edge, so this is also the one place that
/// checks they agree about it — a level whose two halves disagree is not a
/// level, and aggregating it would silently pick one of the two answers.
fn edge_union(cells: &[Cell]) -> BTreeMap<(u32, u32), u64> {
    let mut all: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for (h, cell) in (0u32..).zip(cells) {
        for (&key, &weight) in &cell.edges {
            assert!(
                key.0 == h || key.1 == h,
                "cell {h} carries {key:?}, an edge it is not an endpoint of",
            );
            if let Some(&seen) = all.get(&key) {
                assert_eq!(seen, weight, "the two endpoints of {key:?} disagree");
            }
            all.insert(key, weight);
        }
    }
    all
}

/// **The definition**: what the level below says the level above must hold.
///
/// Members union, counts sum, and the edges — the part that is not a union.
/// An edge between two children of the *same* parent is not an edge of the
/// parent at all; it is absorbed into its `internal` weight. A writer that
/// takes the children's edge set and relabels it keeps those as self-loops or
/// drops them, and either way the aggregate is wrong in a way no count notices.
fn aggregate(children: &[Cell], parents: &[u32], groups: usize) -> Vec<Cell> {
    let mut out = vec![Cell::default(); groups];
    for (child, &parent) in children.iter().zip(parents) {
        let cell = &mut out[parent as usize];
        cell.members.extend(child.members.iter().copied());
        cell.count += child.count;
        cell.internal += child.internal;
    }
    let mut cross: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for ((a, b), weight) in edge_union(children) {
        let (pa, pb) = (parents[a as usize], parents[b as usize]);
        if pa == pb {
            out[pa as usize].internal += weight;
        } else {
            *cross.entry((pa.min(pb), pa.max(pb))).or_default() += weight;
        }
    }
    for (key, weight) in cross {
        *out[key.0 as usize].edges.entry(key).or_default() += weight;
        *out[key.1 as usize].edges.entry(key).or_default() += weight;
    }
    out
}

/// Obligation 1, both directions. Empty means the sets are equal.
fn membership_diff(recorded: &[Cell], defined: &[Cell]) -> Vec<String> {
    let mut findings = width(recorded, defined);
    for (h, (r, d)) in (0u32..).zip(recorded.iter().zip(defined)) {
        let missing: Vec<u32> = d.members.difference(&r.members).copied().collect();
        let invented: Vec<u32> = r.members.difference(&d.members).copied().collect();
        if !missing.is_empty() {
            findings.push(format!(
                "cell {h}: members its children have and it does not: {missing:?}"
            ));
        }
        if !invented.is_empty() {
            findings.push(format!(
                "cell {h}: members it has and no child of it does: {invented:?}"
            ));
        }
    }
    findings
}

/// Obligation 2. Independent of obligation 1 only because the count is a
/// recorded field; see [`Cell`].
fn count_diff(recorded: &[Cell], defined: &[Cell]) -> Vec<String> {
    let mut findings = width(recorded, defined);
    for (h, (r, d)) in (0u32..).zip(recorded.iter().zip(defined)) {
        if r.count != d.count {
            findings.push(format!(
                "cell {h}: records {} members, its children sum to {}",
                r.count, d.count,
            ));
        }
    }
    findings
}

/// Obligation 3, both directions and over weights as well as keys — a quotient
/// with the right neighbours and the wrong weights is a different graph.
fn edge_diff(recorded: &[Cell], defined: &[Cell]) -> Vec<String> {
    let mut findings = width(recorded, defined);
    for (h, (r, d)) in (0u32..).zip(recorded.iter().zip(defined)) {
        for (key, weight) in &d.edges {
            if r.edges.get(key) != Some(weight) {
                findings.push(format!(
                    "cell {h}: {key:?} aggregates to {weight} and it records {:?}",
                    r.edges.get(key),
                ));
            }
        }
        for (key, weight) in &r.edges {
            if !d.edges.contains_key(key) {
                findings.push(format!(
                    "cell {h}: records {key:?} at {weight}, which no child of it has"
                ));
            }
        }
        if r.internal != d.internal {
            findings.push(format!(
                "cell {h}: records {} internal weight, its children aggregate to {}",
                r.internal, d.internal,
            ));
        }
    }
    findings
}

/// The one finding that makes every per-cell comparison below meaningless, so
/// every obligation opens with it: two levels of different widths are not
/// zippable and a zip would quietly check the shorter one.
fn width(recorded: &[Cell], defined: &[Cell]) -> Vec<String> {
    if recorded.len() == defined.len() {
        Vec::new()
    } else {
        vec![format!(
            "{} cells recorded against {} the level below defines",
            recorded.len(),
            defined.len(),
        )]
    }
}

/// Every edge of the graph, counted exactly once from the rows of one level —
/// cross weight plus internal weight. A level that loses an edge somewhere in
/// the aggregation is caught here even if it lost it consistently at both ends.
fn mass(cells: &[Cell]) -> u64 {
    let cross: u64 = edge_union(cells).values().sum();
    cross + cells.iter().map(|h| h.internal).sum::<u64>()
}

/// The recorded and defined sides of one level, which is what every obligation
/// is a comparison of.
struct Pair {
    recorded: Vec<Cell>,
    defined: Vec<Cell>,
}

/// The top of the planted hierarchy: the super-groups, recorded straight from
/// the graph and defined from the cliques underneath them.
fn super_groups() -> Pair {
    let (tree, edges) = stated();
    let cliques = (SUPERS * PER_SUPER) as usize;
    let supers = SUPERS as usize;
    let children = cells_at(
        &tree.membership(0).expect("the clique level"),
        cliques,
        &edges,
    );
    let parents = tree.parents(0, 1).expect("a clique's super-group");
    Pair {
        recorded: cells_at(
            &tree.membership(1).expect("the super level"),
            supers,
            &edges,
        ),
        defined: aggregate(&children, &parents, supers),
    }
}

/// **The ground truth, before either side of a diff is trusted.**
///
/// Every obligation below compares two computations, and two computations can
/// agree on a wrong answer. This is the one test that does not diff: it states
/// what the planting rule puts in each cell — which vertices, how many, which
/// neighbours at what weight — and checks the recorded rows against arithmetic
/// nothing in the crate performed.
#[test]
fn a_planted_two_tier_hierarchy_summarises_exactly_what_construction_says() {
    let (tree, edges) = stated();
    let supers = SUPERS as usize;
    let cells = cells_at(
        &tree.membership(1).expect("the super level"),
        supers,
        &edges,
    );

    // Every edge of the planting rule, by kind: cliques, the links inside a
    // super-group, and the path between them.
    let per_clique = u64::from(SIZE * (SIZE - 1) / 2);
    let per_super = u64::from(PER_SUPER) * per_clique
        + u64::from(PER_SUPER * (PER_SUPER - 1) / 2) * u64::from(LINKS);
    assert_eq!(
        edges.len() as u64,
        u64::from(SUPERS) * per_super + u64::from(SUPERS - 1),
        "the fixture is the graph this test's arithmetic is about",
    );

    for (s, cell) in (0u32..).zip(&cells) {
        let first = s * PER_SUPER * SIZE;
        let expected: BTreeSet<u32> = (first..first + PER_SUPER * SIZE).collect();
        assert_eq!(cell.members, expected, "super-group {s} is its own cliques");
        assert_eq!(cell.count, u64::from(PER_SUPER * SIZE));
        assert_eq!(
            cell.internal, per_super,
            "super-group {s} holds every edge of its cliques and the links between them",
        );
        // The path: one edge to each neighbouring super-group, and nothing else.
        let mut neighbours: BTreeMap<(u32, u32), u64> = BTreeMap::new();
        if s > 0 {
            neighbours.insert((s - 1, s), 1);
        }
        if s + 1 < SUPERS {
            neighbours.insert((s, s + 1), 1);
        }
        assert_eq!(cell.edges, neighbours, "super-group {s}'s own edges");
    }
    assert_eq!(mass(&cells), edges.len() as u64, "every edge, once");
}

/// **Obligation 1** — and the corruption it exists for.
///
/// The red half swaps one member between two cells. Both counts are still
/// right, both cells still hold the number of vertices their children add up
/// to, and the tree summarises a graph in which two vertices changed community.
/// That is the case a count cannot see and the reason the comparison is a
/// symmetric difference of sets.
#[test]
fn a_cells_members_are_exactly_the_children_that_name_it_as_parent() {
    let Pair { recorded, defined } = super_groups();
    assert!(
        membership_diff(&recorded, &defined).is_empty(),
        "{:?}",
        membership_diff(&recorded, &defined),
    );

    // The corruption is applied to the recorded rows themselves; the defined
    // side stays exactly what the level below says it must be.
    let mut swapped = recorded;
    let (a, b) = (
        *swapped[0].members.iter().next().expect("a member"),
        *swapped[1].members.iter().next().expect("a member"),
    );
    swapped[0].members.remove(&a);
    swapped[0].members.insert(b);
    swapped[1].members.remove(&b);
    swapped[1].members.insert(a);

    assert!(
        count_diff(&swapped, &defined).is_empty(),
        "the swap is invisible to obligation 2, which is the point of it",
    );
    assert_eq!(
        membership_diff(&swapped, &defined).len(),
        4,
        "one vertex missing and one invented, in each of two cells",
    );
}

/// **Obligation 2** — the count a writer records beside the members, not the
/// length of the set it recorded.
///
/// The red half leaves every member set exactly right and adds one to a count,
/// which is the shape of every count that was computed once and reused after
/// the thing it counted changed.
#[test]
fn a_cells_count_is_the_sum_of_its_childrens_counts() {
    let Pair { recorded, defined } = super_groups();
    assert!(
        count_diff(&recorded, &defined).is_empty(),
        "{:?}",
        count_diff(&recorded, &defined),
    );

    let mut miscounted = recorded;
    miscounted[3].count += 1;
    assert!(
        membership_diff(&miscounted, &defined).is_empty(),
        "the member sets are untouched",
    );
    assert_eq!(count_diff(&miscounted, &defined).len(), 1);
}

/// **Obligation 3** — and the half of it that is not a union.
///
/// Aggregating edges is where a writer loses mass: an edge between two children
/// of one parent stops being an edge and becomes the parent's own internal
/// weight, and here that is most of the graph — 1,088 of 1,095 edges are
/// internal to a super-group. The red half drops one edge from one endpoint,
/// which is what a relabelling that deduplicates by key looks like from outside.
#[test]
fn a_cells_edges_are_the_aggregation_of_its_childrens_edges() {
    let Pair { recorded, defined } = super_groups();
    assert!(
        edge_diff(&recorded, &defined).is_empty(),
        "{:?}",
        edge_diff(&recorded, &defined),
    );

    let (_, edges) = planted();
    assert_eq!(
        mass(&recorded),
        edges.len() as u64,
        "no edge lost, none made"
    );
    assert_eq!(mass(&recorded), mass(&defined));
    // The absorbed edges, positively: the cliques inside a super-group are
    // joined, and none of those joins survives as an edge of the cell.
    let internal: u64 = recorded.iter().map(|h| h.internal).sum();
    assert_eq!(internal, edges.len() as u64 - u64::from(SUPERS - 1));

    let mut lost = recorded;
    let key = *lost[0].edges.keys().next().expect("the path edge");
    lost[0].edges.remove(&key);
    let findings = edge_diff(&lost, &defined);
    assert_eq!(findings.len(), 1, "{findings:?}");
}

/// **The three, up every rung of a cut nobody designed.**
///
/// The tests above run over the partition the fixture was planted with, which
/// is the partition a reader of this file can check by eye. This one runs over
/// what Louvain returned and `Dendrogram::cut` published out of it: the levels
/// are whatever the algorithm found, the parent columns are
/// `Dendrogram::parents`, and the chain starts at the **vertices** — the finest
/// rung is defined against the graph itself, so nothing between the edge list
/// and the top of the tree is taken on trust.
#[test]
fn the_three_obligations_hold_up_a_cut_of_a_partition_nobody_designed() {
    let (vertex_count, edges) = planted();
    let tree = Dendrogram::of(vertex_count, &edges);
    let cut = tree.cut(4);
    assert!(
        cut.len() >= 2,
        "a two-tier graph cuts to two rungs: {cut:?}"
    );

    let findings = obligations_up(&tree, &cut, &edges);
    assert!(findings.is_empty(), "{findings:#?}");
}

/// Every rung of a cut against the level below it, the finest against the
/// vertices themselves.
///
/// Each rung is checked against the rows the rung **below it published**, not
/// against the aggregate that was derived for it — a tree is read by descending
/// from one published level to the next, so that is the relation that has to
/// hold.
fn obligations_up(tree: &Dendrogram, cut: &Cut, edges: &[(u32, u32)]) -> Vec<String> {
    let vertex_count = tree.vertex_count();
    let identity: Vec<u32> = (0..vertex_count).collect();
    let mut children = cells_at(&identity, vertex_count as usize, edges);
    let mut below: Option<usize> = None;
    let mut findings = Vec::new();

    for rung in cut.rungs() {
        let membership = tree.membership(rung.level()).expect("a rung is a level");
        let groups = rung.groups() as usize;
        let parents = below.map_or_else(
            // The finest rung's children are the vertices, and a vertex's
            // parent is its own group id.
            || membership.clone(),
            |fine| {
                tree.parents(fine, rung.level())
                    .expect("a rung above a rung")
            },
        );
        let recorded = cells_at(&membership, groups, edges);
        let defined = aggregate(&children, &parents, groups);

        let level = rung.level();
        for finding in membership_diff(&recorded, &defined) {
            findings.push(format!("rung at level {level}, members: {finding}"));
        }
        for finding in count_diff(&recorded, &defined) {
            findings.push(format!("rung at level {level}, counts: {finding}"));
        }
        for finding in edge_diff(&recorded, &defined) {
            findings.push(format!("rung at level {level}, edges: {finding}"));
        }
        if mass(&recorded) != edges.len() as u64 {
            findings.push(format!(
                "rung at level {level}: {} edge weight over a graph of {}",
                mass(&recorded),
                edges.len(),
            ));
        }

        children = recorded;
        below = Some(rung.level());
    }
    findings
}

/// Determinism is the property `/docs/design/cells` puts first, and it is a
/// property of the rows and not only of the partition: the same corpus gives
/// the same cells, bit for bit, or a bookmark does not resolve tomorrow.
///
/// `tests/dendrogram.rs` holds the cut to that standard. This holds the rows to
/// it — two independent evaluations, nothing shared, compared whole. It is the
/// assertion that nothing in the aggregation above reached for a hash.
#[test]
fn the_same_graph_gives_the_same_cell_rows_twice() {
    let first = rows_of_every_rung();
    let second = rows_of_every_rung();
    assert_eq!(first, second);
    assert!(!first.is_empty(), "a cut with no rungs proves nothing");
}

fn rows_of_every_rung() -> Vec<Vec<Cell>> {
    let (vertex_count, edges) = planted();
    let tree = Dendrogram::of(vertex_count, &edges);
    tree.cut(4)
        .rungs()
        .iter()
        .map(|rung| {
            let membership = tree.membership(rung.level()).expect("a rung is a level");
            cells_at(&membership, rung.groups() as usize, &edges)
        })
        .collect()
}

/// **And the one thing above that is not checked.**
///
/// Counts sum and edges aggregate; where a cell goes on the plane is a choice,
/// and there is no diff that makes a choice correct. So the obligation on a
/// position is not that it verifies — it is that it is **declared**, in the same
/// field a vertex's position is declared in and with the same three answers
/// available: `crates/fossil-sinks/src/manifest.rs, Provenance`.
///
/// A cell's position is `Derived` by construction — something chose it because
/// a picture needed coordinates — so its density is a fact about the choosing.
/// What makes the declaration worth anything is `derived_by`: re-deriving is how
/// a reader checks that a cell's referent has not moved, and nothing can re-run
/// what nothing names. `None` there is a weaker statement than a name, not an
/// equivalent one, which is why this asserts the name is present rather than
/// asserting the position.
#[test]
fn a_cells_position_is_declared_rather_than_derived_in_silence() {
    let declared = CoordinateSystem::derived("cell", "x", "y", "louvain-cut+member-centroid");
    assert_eq!(declared.provenance, Provenance::Derived);
    assert!(
        !declared.is_data(),
        "a chosen position is not a measurement of anything",
    );
    assert!(
        declared.derived_by.is_some(),
        "a derived system that names nothing cannot be re-derived",
    );
    // The distinction the declaration exists to carry: these are two float32
    // columns either way, and nothing but this field tells them apart.
    let measured = CoordinateSystem::measured("geo", "lon", "lat", Provenance::Geographic);
    assert!(measured.is_data());
    assert_ne!(declared.provenance, measured.provenance);
}
