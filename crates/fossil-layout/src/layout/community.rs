//! The partition: modularity communities and the whole hierarchy of them, plus
//! the reachability it is contrasted against.
//!
//! Split out of `layout.rs` unchanged. [`community_hierarchy`] is the dendrogram
//! the level-of-detail plan is built from; [`weakly_connected_components`] is the
//! contrast it is tested against, and no longer what the layout uses.
//!
//! The output is reproducible, and that is load-bearing rather than incidental —
//! `/docs/design/holons` is the page that depends on it and carries the
//! measurement.

use fossil_sinks::manifest::VertexLevels;

/// Assign each vertex `0..vertex_count` a dense, contiguous `cluster_id` via
/// weakly-connected components (union-find with path halving).
///
/// `edges` are `(src_dense, dst_dense)` pairs; direction is ignored (weak
/// connectivity). Out-of-range endpoints are skipped (defensive — a clean
/// writer never emits them). Cluster ids are relabelled to `0..k` in order of
/// first appearance by ascending `dense_id`, so the numbering is stable and
/// gap-free regardless of union order.
#[must_use]
pub fn weakly_connected_components(vertex_count: u32, edges: &[(u32, u32)]) -> Vec<u32> {
    let n = vertex_count as usize;
    let mut parent: Vec<u32> = (0..vertex_count).collect();

    for &(a, b) in edges {
        if (a as usize) < n && (b as usize) < n {
            let ra = find(&mut parent, a);
            let rb = find(&mut parent, b);
            if ra != rb {
                parent[ra as usize] = rb;
            }
        }
    }

    // Relabel roots to dense 0..k in first-appearance order.
    let mut label: Vec<Option<u32>> = vec![None; n];
    let mut next = 0u32;
    let mut out = vec![0u32; n];
    for i in 0..n {
        let root = find(&mut parent, i as u32);
        let id = if let Some(id) = label[root as usize] {
            id
        } else {
            let id = next;
            label[root as usize] = Some(id);
            next += 1;
            id
        };
        out[i] = id;
    }
    out
}

/// Union-find root with path halving (iterative — no recursion, no stack risk
/// on million-node chains).
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let grand = parent[parent[x as usize] as usize];
        parent[x as usize] = grand;
        x = grand;
    }
    x
}

/// Modularity-based community detection, returning the **whole hierarchy**
/// rather than one partition.
///
/// This is what [`weakly_connected_components`] cannot give. WCC is a
/// reachability partition, so on a connected graph it answers "one community"
/// — measured on a benchmark corpus it put 1,998 of 2,000 vertices in a single
/// cluster, and a viewport window then retained **375 of 27,244
/// incident edges (1.4%)**, barely four times chance. Positions derived from
/// that partition are topology-blind, and a spatial index over topology-blind
/// positions is fast access to noise.
///
/// The hierarchy is the point, not a by-product: level *l* is a graph whose
/// nodes are level *l−1*'s communities, which is exactly the quotient graph a
/// level-of-detail pyramid needs. `contract` stops being a query verb and
/// becomes how a level is built.
///
/// # What this is and is not
///
/// This is **Louvain** (local moving + aggregation, maximising modularity), not
/// Leiden. Leiden adds a *refinement* pass that guarantees every community is
/// internally well-connected; Louvain can emit communities that are internally
/// disconnected, which for a viewport shows up as a "community" whose members
/// sit together on screen while having no path between them. Naming it honestly
/// matters more than claiming the better algorithm: the refinement pass is a
/// separate slice, and it slots in between the local-moving loop and the
/// aggregation below without changing this signature.
///
/// Deterministic: nodes are visited in index order and ties are broken by lower
/// community id, so the same input yields the same hierarchy on every run — no
/// RNG, like everything else in this module.
///
/// # Returns
///
/// One entry per level. `levels[0][v]` is the community of original vertex `v`;
/// `levels[l][c]` is the parent community of level-`l` community `c`. Community
/// ids at every level are dense (`0..k`). The vector is empty for an empty
/// graph, and stops as soon as a pass merges nothing.
#[must_use]
pub fn community_hierarchy(vertex_count: u32, edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    if vertex_count == 0 {
        return Vec::new();
    }
    hierarchy(Weighted::from_edges(vertex_count, edges))
}

/// [`community_hierarchy`] over a graph that is already built, which is how the
/// write path enters: it reads the two orientations the artefact stores and has
/// no bag of pairs to hand over.
pub(super) fn hierarchy(mut graph: Weighted) -> Vec<Vec<u32>> {
    let mut levels: Vec<Vec<u32>> = Vec::new();
    loop {
        let membership = local_moving(&graph);
        let community_count = membership.iter().copied().max().map_or(0, |m| m + 1);
        // A pass that merges nothing is where the hierarchy ends: recording it
        // would add a level that is a copy of the one below.
        if community_count as usize == graph.node_count() {
            break;
        }
        graph = graph.contract(&membership, community_count);
        levels.push(membership);
        if community_count <= 1 {
            break;
        }
    }
    levels
}

/// Collapse a hierarchy to one community per vertex, taking the **finest level
/// that fits `budget`**.
///
/// A hierarchy has no single "the" partition, so something has to choose, and
/// the choice is not free in either direction. Too coarse and a community is
/// larger than any window, so grouping by it buys a camera nothing. Too fine and
/// an aggregate read, which answers one super-node per cluster under a row cap,
/// starts dropping communities off the end of the list rather than reporting
/// that it did — see [`CLUSTER_BUDGET`].
///
/// Levels compose by lookup, not by recomputation: level *l* is indexed by level
/// *l−1*'s community ids, so walking up is `c ← levels[l][c]`. The walk stops at
/// the first level within budget, and at the top level regardless — the coarsest
/// level is the smallest there is, so if it still exceeds the budget there is
/// nothing better to return.
///
/// An empty hierarchy means nothing merged (a graph with no edges), and then
/// every vertex is its own community, which is the truth about that graph.
/// Returns the membership and **which level it came from**, because the level is
/// what the placement above it still needs in order to know who is whose sibling.
#[must_use]
pub(super) fn flatten_to_budget(
    levels: &[Vec<u32>],
    vertex_count: u32,
    budget: u32,
) -> (Vec<u32>, Option<usize>) {
    let Some(finest) = levels.first() else {
        return ((0..vertex_count).collect(), None);
    };
    let count = |m: &[u32]| m.iter().copied().max().map_or(0, |x| x + 1);

    let mut current = finest.clone();
    let mut chosen = 0;
    for level in &levels[1..] {
        if count(&current) <= budget {
            break;
        }
        current = current.iter().map(|&c| level[c as usize]).collect();
        chosen += 1;
    }
    (current, Some(chosen))
}

/// Renumber the chosen level's communities so that **siblings are consecutive**,
/// by sorting each community on the path of ancestors above it.
///
/// The number a community wears decides where [`cluster_layout`] puts it, and
/// until now that number came from the order vertices happened to be visited in
/// — which is `dense_id` order, which is IRI order, which has nothing to do with
/// the graph. Two communities with thousands of edges between them therefore
/// landed on opposite sides of the picture as often as not, and every one of
/// those edges left whatever window either of them was in.
///
/// Sorting by the ancestor path *is* a depth-first walk of the hierarchy, so no
/// tree is built to do it: level *l+1* maps a community to its parent, and
/// following that up to the top gives a key whose lexicographic order visits
/// each subtree contiguously. The community's own id goes last so the order is
/// total, and therefore reproducible.
///
/// This orders the communities. It does not lay out each level's quotient graph
/// — a community's *position among its siblings* is still its id and not its
/// connections, so this buys locality between subtrees, not within one.
pub(super) fn order_by_hierarchy(levels: &[Vec<u32>], chosen: usize, membership: &mut [u32]) {
    let cluster_count = membership.iter().copied().max().map_or(0, |m| m + 1);
    if cluster_count == 0 {
        return;
    }
    let above = &levels[chosen + 1..];

    let mut keyed: Vec<(Vec<u32>, u32)> = (0..cluster_count)
        .map(|c| {
            let mut path = Vec::with_capacity(above.len() + 1);
            let mut current = c;
            for level in above {
                current = level[current as usize];
                path.push(current);
            }
            // Coarsest ancestor first, so the sort groups whole subtrees before
            // it ever looks at a finer distinction.
            path.reverse();
            path.push(c);
            (path, c)
        })
        .collect();
    keyed.sort_unstable();

    let mut rank = vec![0u32; cluster_count as usize];
    for (r, (_, c)) in keyed.iter().enumerate() {
        rank[*c as usize] = r as u32;
    }
    for m in membership.iter_mut() {
        *m = rank[*m as usize];
    }
}

/// What a half-edge weighs.
///
/// Level 0 is the graph the artefact stores and every edge there weighs one, so
/// the array is not stored at all; only [`Weighted::contract`] sums weights and
/// therefore has to keep them. Measured at ten million, the `f64` per half-edge
/// was 1,136 MB — 16 of the 53 B/edge the core costs.
enum Weights {
    Unit,
    Stored(Vec<f64>),
}

impl Weights {
    fn at(&self, i: usize) -> f64 {
        match self {
            Self::Unit => 1.0,
            Self::Stored(w) => w[i],
        }
    }

    /// Total weight over `range`, which for [`Self::Unit`] is its length: the
    /// sum of `k` ones is exactly `k` in `f64`, so this is the same number the
    /// stored variant produces and not an approximation of it.
    fn sum(&self, range: std::ops::Range<usize>) -> f64 {
        match self {
            Self::Unit => range.len() as f64,
            Self::Stored(w) => w[range].iter().sum(),
        }
    }
}

/// One orientation of an adjacency in compressed row form: vertex `v`'s
/// neighbours under this orientation are `targets[offsets[v]..offsets[v + 1]]`.
pub(super) struct Csr {
    offsets: Vec<usize>,
    targets: Vec<u32>,
    weights: Weights,
}

impl Csr {
    fn range(&self, v: usize) -> std::ops::Range<usize> {
        self.offsets[v]..self.offsets[v + 1]
    }

    fn neighbours(&self, v: usize) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.range(v).map(|i| (self.targets[i], self.weights.at(i)))
    }
}

/// Build a [`Csr`] from rows that already arrive grouped by their key.
///
/// A run of equal keys **is** a vertex's neighbour list, so the offsets are
/// written as the runs close and nothing is counted twice or written out of
/// place. That is the whole of it: the degree pre-pass and the
/// cursor scatter [`Weighted::from_edges`] needs exist only because its
/// parameter is an unordered bag, and the file on disk has never been one.
pub(super) struct CsrBuilder {
    n: usize,
    offsets: Vec<usize>,
    targets: Vec<u32>,
    /// The vertex whose run is open. Every key seen so far is `<=` it, which is
    /// what makes a file that lies about its order detectable in one comparison.
    open: usize,
}

impl CsrBuilder {
    pub(super) fn new(n: usize, edges: usize) -> Self {
        let mut offsets = Vec::with_capacity(n + 1);
        offsets.push(0);
        Self {
            n,
            offsets,
            targets: Vec::with_capacity(edges),
            open: 0,
        }
    }

    /// One Arrow batch, as the two columns it already is. `false` means the keys
    /// went backwards, i.e. the file is not what it declares itself to be.
    pub(super) fn push(&mut self, keys: &[u32], values: &[u32], self_loops: &mut [f64]) -> bool {
        for (&key, &value) in keys.iter().zip(values) {
            let (key, value) = (key as usize, value as usize);
            if key < self.open {
                return false;
            }
            // Keys ascend, so the first one past the last vertex ends the useful
            // part of the file. A clean writer emits none of these.
            if key >= self.n {
                break;
            }
            while self.open < key {
                self.open += 1;
                self.offsets.push(self.targets.len());
            }
            if value == key {
                self_loops[key] += 1.0;
            } else if value < self.n {
                self.targets.push(value as u32);
            }
        }
        true
    }

    pub(super) fn finish(mut self) -> Csr {
        while self.open < self.n {
            self.open += 1;
            self.offsets.push(self.targets.len());
        }
        Csr {
            offsets: self.offsets,
            targets: self.targets,
            weights: Weights::Unit,
        }
    }
}

/// An undirected weighted graph, as however many oriented adjacencies it was
/// read from, with self-loops kept apart.
///
/// `sides` is a list rather than one array because that is what the artefact
/// hands over: a source-ordered file and a target-ordered file per edge table,
/// each already grouped by the endpoint it is ordered on. Merging them into one
/// adjacency would be a copy of the whole graph to buy nothing — a vertex's
/// neighbourhood is the concatenation of its run in each. A contraction builds
/// one symmetric side and so has a list of one.
///
/// Self-loops are separate because aggregation creates them — a community's
/// internal edges become one — and because they enter the degree twice while
/// appearing once in the adjacency. Folding them into `targets` would make every
/// later sum quietly wrong by a factor of two.
pub(super) struct Weighted {
    sides: Vec<Csr>,
    /// Weight of each node's self-loop, counted **once**.
    self_loops: Vec<f64>,
    /// Sum of incident weights plus twice the self-loop — the `k_i` of the
    /// modularity formula.
    degrees: Vec<f64>,
    /// Total edge weight `m`, i.e. half the sum of all degrees.
    total: f64,
}

impl Weighted {
    const fn node_count(&self) -> usize {
        self.self_loops.len()
    }

    /// Build from an unweighted, possibly duplicated edge list. Parallel edges
    /// add their weights rather than being deduplicated: two links between the
    /// same pair really are a stronger tie, and modularity is defined over
    /// weights.
    ///
    /// This is what an unordered bag costs — a degree pass, a prefix sum and a
    /// scatter through a cloned cursor — and it is kept for callers that have
    /// one, which now means the tests and
    /// `examples/layout_memory.rs`. The write path reads [`Csr`]s instead.
    fn from_edges(vertex_count: u32, edges: &[(u32, u32)]) -> Self {
        let n = vertex_count as usize;
        let mut degree_count = vec![0usize; n];
        for &(a, b) in edges {
            if (a as usize) < n && (b as usize) < n && a != b {
                degree_count[a as usize] += 1;
                degree_count[b as usize] += 1;
            }
        }
        let mut offsets = Vec::with_capacity(n + 1);
        let mut acc = 0usize;
        offsets.push(0);
        for d in &degree_count {
            acc += *d;
            offsets.push(acc);
        }
        let mut cursor = offsets.clone();
        let mut targets = vec![0u32; acc];
        let mut self_loops = vec![0.0f64; n];
        for &(a, b) in edges {
            if (a as usize) >= n || (b as usize) >= n {
                continue;
            }
            if a == b {
                self_loops[a as usize] += 1.0;
                continue;
            }
            targets[cursor[a as usize]] = b;
            cursor[a as usize] += 1;
            targets[cursor[b as usize]] = a;
            cursor[b as usize] += 1;
        }
        Self::finish(
            vec![Csr {
                offsets,
                targets,
                weights: Weights::Unit,
            }],
            self_loops,
        )
    }

    pub(super) fn finish(sides: Vec<Csr>, self_loops: Vec<f64>) -> Self {
        let n = self_loops.len();
        let mut degrees = vec![0.0f64; n];
        for v in 0..n {
            let incident: f64 = sides.iter().map(|s| s.weights.sum(s.range(v))).sum();
            degrees[v] = 2.0f64.mul_add(self_loops[v], incident);
        }
        let total = degrees.iter().sum::<f64>() / 2.0;
        Self {
            sides,
            self_loops,
            degrees,
            total,
        }
    }

    fn neighbours(&self, v: usize) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.sides.iter().flat_map(move |side| side.neighbours(v))
    }

    /// The quotient graph: one node per community, intra-community weight
    /// folded into a self-loop, inter-community weight summed.
    ///
    /// # This is where the layout pass's memory was
    ///
    /// One row of the quotient was one `HashMap<u32, f64>`, and there is one row
    /// per community — 748,647 of them at two million vertices and **3,757,900**
    /// at ten, because the *first* contraction is the one that barely contracts:
    /// level zero stops on the 32-sweep cap without converging, and the ten
    /// thousand planted communities of the fixture are not found until level one.
    ///
    /// Sampled per step **inside** the hierarchy rather than at the phase
    /// boundary, that array of maps is **+3.41 GiB of the +3.82 GiB**
    /// `community_hierarchy` bills at ten million, and the quotient CSR built out
    /// of it is another +1.04. It holds 110,070,012 entries between its 3,757,900
    /// tables — about 33 bytes per surviving inter-community half-edge, which is
    /// a `(u32, f64)` bucket plus its control byte plus what rounding a table up
    /// to a power of two costs. Beside it, `local_moving`'s resident set is flat
    /// to three decimal places across all thirty-two sweeps.
    ///
    /// So `community_hierarchy`'s bill was never the hundreds of millions of
    /// transient maps it was read as. It was this one array of long-lived ones.
    ///
    /// (`examples/enrich_memory 10000000 14`, 2026-08-28, with a per-level probe
    /// compiled in — which costs that run about 40 s and 0.3 GiB of its own, so
    /// the +3.82 above is its `community_hierarchy` and not the 228.2 s / +3.47
    /// GiB the same build measures without it.)
    ///
    /// So the maps are gone and nothing replaces them. **Group the nodes by
    /// community first** — a counting sort, `n` `u32` and two arrays of `k` —
    /// then build one row at a time into the same sparse accumulator
    /// [`local_moving`] uses, emitting it into the CSR before the next row
    /// starts. What was `k` hash tables live at once is now one dense array of
    /// `k`, and the quotient's own `targets`/`weights` are the only thing that
    /// scales with the surviving edges.
    ///
    /// # Why the output is bit-identical
    ///
    /// The counting sort is stable by construction — nodes are counted and
    /// scattered in ascending order — so a community's members are visited in
    /// exactly the order the `0..n` loop visited them. Every `f64` in
    /// `self_loops` and in `weights` is therefore the same sequence of additions
    /// as before, and the rows come out sorted by community id, which is what
    /// the `entries.sort_unstable_by_key` it replaces was for.
    fn contract(&self, membership: &[u32], community_count: u32) -> Self {
        let k = community_count as usize;
        let n = self.node_count();

        // Members of each community, ascending, as a counting sort: `starts` is
        // the prefix sum of the community sizes and `members` the nodes laid out
        // under it.
        let mut starts = vec![0u32; k + 1];
        for &c in &membership[..n] {
            starts[c as usize + 1] += 1;
        }
        for c in 0..k {
            starts[c + 1] += starts[c];
        }
        let mut members = vec![0u32; n];
        {
            let mut cursor = starts.clone();
            for (v, &c) in membership[..n].iter().enumerate() {
                let c = c as usize;
                members[cursor[c] as usize] = v as u32;
                cursor[c] += 1;
            }
        }

        let mut self_loops = vec![0.0f64; k];
        let mut offsets = Vec::with_capacity(k + 1);
        let mut targets: Vec<u32> = Vec::new();
        let mut weights: Vec<f64> = Vec::new();
        offsets.push(0);
        let mut row = Neighbourhood::new(k);
        for c in 0..k {
            row.clear();
            for &v in &members[starts[c] as usize..starts[c + 1] as usize] {
                let v = v as usize;
                // Each node's own self-loop carries over whole.
                self_loops[c] += self.self_loops[v];
                for (u, w) in self.neighbours(v) {
                    let cu = membership[u as usize];
                    if cu as usize == c {
                        // Counted once per direction, so half lands here and
                        // half when the other endpoint is visited.
                        self_loops[c] += w / 2.0;
                    } else {
                        row.add(cu, w);
                    }
                }
            }
            // Sorted so the structure is a pure function of the input, not of
            // the order the neighbours happened to arrive in.
            row.sort();
            for &cu in row.communities() {
                targets.push(cu);
                weights.push(row.weight_of(cu));
            }
            offsets.push(targets.len());
        }
        Self::finish(
            vec![Csr {
                offsets,
                targets,
                weights: Weights::Stored(weights),
            }],
            self_loops,
        )
    }
}

/// The weight from one node into each of its neighbouring communities, as **one
/// allocation reused by every node of every sweep**.
///
/// A `HashMap` built and dropped per node per sweep is what stood here, and at
/// ten million vertices that is a few hundred million transient allocations —
/// [`local_moving`] is 32 sweeps over ten million nodes, because level zero
/// never converges and stops on the sweep cap. It cost **no resident memory at
/// all**: measured per sweep, RSS is flat to three decimal places across all
/// thirty-two. What it cost was time.
///
/// So this is a wall-clock change and it is honest about being one: three arrays
/// of `n`, 90 MB at ten million, for **Louvain 228.2 s → 132.8 s** — 1.72×,
/// measured with this change alone and nothing else in the tree, against a
/// process peak that goes the wrong way by 0.36 GiB (8.54 → 8.90).
///
/// That trade was refused once and the refusal was correct at the time: it spent
/// the one quantity `--memory-gib` bounds to buy wall clock that was not the
/// objective. What removed the objection was [`Weighted::contract`], after which
/// the peak was 5.08 GiB and 90 MB was not a trade; the adjacency remap has
/// taken it to **3.94** since, and 90 MB is less of one still.
///
/// (`examples/enrich_memory 10000000 14`, 2026-08-28, Mac16,8 — 14 cores,
/// 48 GiB, macOS 26.2 / Darwin 25.2.0.)
///
/// # Why the output is bit-identical, and not merely equal
///
/// Two orders decide the answer and both are preserved. **The accumulation
/// order** is the neighbour walk, unchanged, so every `f64` sum is the same
/// sequence of additions and therefore the same bits. **The comparison order**
/// is ascending community id — the `HashMap` version collected its entries into
/// a `Vec` and sorted it for exactly this reason — so [`Self::sort`] sorts
/// `touched` and the strictly-greater tie-break keeps the same winner.
///
/// [`Self::clear`] walks `touched` rather than the whole array, so a sweep is
/// O(E) and not O(V·k): the cost of resetting a node's accumulator is its
/// degree.
struct Neighbourhood {
    /// Accumulated weight per community id. Only the entries `touched` names are
    /// meaningful; every other entry is `0.0` and [`Self::clear`] keeps it so.
    weight: Vec<f64>,
    /// Whether a community id is already in `touched`, so a second edge into it
    /// does not list it twice.
    listed: Vec<bool>,
    /// The communities this node has an edge into. Cleared per node.
    touched: Vec<u32>,
}

impl Neighbourhood {
    /// Sized by the node count rather than by the community count, because a
    /// community id **is** a node id: [`local_moving`] starts every node in its
    /// own community and only ever moves it into one that already exists.
    fn new(n: usize) -> Self {
        Self {
            weight: vec![0.0; n],
            listed: vec![false; n],
            touched: Vec::new(),
        }
    }

    fn clear(&mut self) {
        for &c in &self.touched {
            self.weight[c as usize] = 0.0;
            self.listed[c as usize] = false;
        }
        self.touched.clear();
    }

    fn add(&mut self, community: u32, weight: f64) {
        let i = community as usize;
        if !self.listed[i] {
            self.listed[i] = true;
            self.touched.push(community);
        }
        self.weight[i] += weight;
    }

    /// `0.0` for a community with no edge into it, which is what
    /// `HashMap::get(..).unwrap_or(0.0)` said and what the gain formula wants:
    /// the node's own community is compared whether or not it is a neighbour.
    fn weight_of(&self, community: u32) -> f64 {
        self.weight[community as usize]
    }

    fn sort(&mut self) {
        self.touched.sort_unstable();
    }

    fn communities(&self) -> &[u32] {
        &self.touched
    }
}

/// One Louvain local-moving pass: repeatedly move each node to the neighbouring
/// community that most increases modularity, until a sweep moves nothing.
///
/// The gain of moving isolated node `i` into community `C` is proportional to
/// `k_i_in - Σ_tot(C)·k_i / (2m)`; the constant factor is the same for every
/// candidate, so only the comparison matters and it is never divided out.
fn local_moving(graph: &Weighted) -> Vec<u32> {
    let n = graph.node_count();
    let mut community: Vec<u32> = (0..n as u32).collect();
    if graph.total <= 0.0 {
        // No edges: every node is its own community and nothing can improve.
        return community;
    }
    // Σ_tot per community — the sum of degrees of its members.
    let mut totals: Vec<f64> = graph.degrees.clone();
    let two_m = 2.0 * graph.total;

    // One allocation for the whole call — see [`Neighbourhood`].
    let mut into = Neighbourhood::new(n);

    let mut moved = true;
    let mut sweeps = 0;
    // Bounded because a pathological tie could otherwise oscillate. It is not a
    // safety net at level zero: on the ten-million fixture the first level runs
    // all thirty-two and is still moving nodes when it stops.
    while moved && sweeps < 32 {
        moved = false;
        sweeps += 1;
        for v in 0..n {
            let own = community[v];
            let k_v = graph.degrees[v];
            // Weight from v into each neighbouring community, into the one
            // accumulator this whole call shares.
            into.clear();
            for (u, w) in graph.neighbours(v) {
                into.add(community[u as usize], w);
            }
            // Remove v from its community before comparing, so staying put is
            // evaluated on the same footing as moving.
            totals[own as usize] -= k_v;

            let mut best = own;
            let mut best_gain = totals[own as usize].mul_add(-k_v / two_m, into.weight_of(own));
            // Ascending community id, which is what decides ties below.
            into.sort();
            for &c in into.communities() {
                if c == own {
                    continue;
                }
                let gain = totals[c as usize].mul_add(-k_v / two_m, into.weight_of(c));
                // Strictly greater keeps the lower community id on a tie, which
                // is what makes the result reproducible.
                if gain > best_gain {
                    best_gain = gain;
                    best = c;
                }
            }
            totals[best as usize] += k_v;
            if best != own {
                community[v] = best;
                moved = true;
            }
        }
    }
    densify(&mut community);
    community
}

/// Relabel community ids to `0..k` in order of first appearance, so every level
/// hands the next one a dense node range.
fn densify(community: &mut [u32]) {
    let mut seen: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for c in community.iter_mut() {
        let next = seen.len() as u32;
        *c = *seen.entry(*c).or_insert(next);
    }
}

/// **The dendrogram, addressable.**
///
/// [`community_hierarchy`] hands back the levels Louvain happened to stop at,
/// and the write path reads exactly two of them — the finest as the placement,
/// and the one [`flatten_to_budget`] picks as `cluster_id` — freeing everything
/// between. That is why a caller wanting to cut the tree at chosen sizes, which
/// is what `/docs/design/holons` asks for, had nowhere to reach: the levels are
/// a `Vec<Vec<u32>>` with the arithmetic for walking them living in whichever
/// function needed it.
///
/// This owns the levels and answers the three questions that walk is for: how
/// many groups a level has ([`Self::group_counts`]), which group a vertex is in
/// at that level ([`Self::membership`]), and which group of a coarser level is
/// a group's parent ([`Self::parents`]). [`Self::cut`] is the one that chooses.
///
/// # Determinism
///
/// Nothing here decides anything Louvain did not already decide. Every method
/// is a lookup walk — `c ← levels[l][c]`, over `Vec`s, in `0..n` index order —
/// so the reproducibility `local_moving` and [`Weighted::contract`] argue for is
/// inherited whole: no hash iteration in any output path, no floating point at
/// all outside [`Cut::contractions`], which only reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dendrogram {
    vertex_count: u32,
    levels: Vec<Vec<u32>>,
    /// Groups per level — `max + 1`, in the same order as `levels`. Computed
    /// once at construction because [`Self::cut`] asks for it per rung and a
    /// level is up to `vertex_count` entries long.
    counts: Vec<u32>,
}

/// What stops a `Vec<Vec<u32>>` from being a dendrogram.
///
/// One variant, because one property is enough: level `l + 1` is **indexed by**
/// level `l`'s group ids, so if the widths stack then every composition walk in
/// [`Dendrogram`] is in range, and if they do not then the first one panics. A
/// public constructor that takes levels from outside checks this rather than
/// indexing on trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DendrogramError {
    /// A level is not indexed by the one below it.
    #[error(
        "level {level} is indexed by {found} nodes, but the level below it has {expected} groups"
    )]
    Unstacked {
        /// The offending level's index in the hierarchy, `0` being the finest.
        level: usize,
        /// How many entries that level actually has.
        found: usize,
        /// How many it must have: the group count of the level below, or the
        /// vertex count at level `0`.
        expected: usize,
    },
}

impl Dendrogram {
    /// Run Louvain over an edge list and keep the whole hierarchy.
    ///
    /// The same call as [`community_hierarchy`], which stays and stays the
    /// shape it is — the levels themselves are still what the write path wants.
    #[must_use]
    pub fn of(vertex_count: u32, edges: &[(u32, u32)]) -> Self {
        Self::trusted(vertex_count, community_hierarchy(vertex_count, edges))
    }

    /// Adopt levels that already exist — a hierarchy read back from a corpus, a
    /// partition produced by something other than Louvain, or a stated one in a
    /// test.
    ///
    /// Checks only that the levels stack, which is the property every walk here
    /// indexes on.
    pub fn new(vertex_count: u32, levels: Vec<Vec<u32>>) -> Result<Self, DendrogramError> {
        let mut expected = vertex_count as usize;
        for (level, members) in levels.iter().enumerate() {
            if members.len() != expected {
                return Err(DendrogramError::Unstacked {
                    level,
                    found: members.len(),
                    expected,
                });
            }
            expected = group_count(members) as usize;
        }
        Ok(Self::trusted(vertex_count, levels))
    }

    /// Levels this module produced itself, whose stacking is
    /// [`hierarchy`]'s own invariant.
    fn trusted(vertex_count: u32, levels: Vec<Vec<u32>>) -> Self {
        let counts = levels.iter().map(|l| group_count(l)).collect();
        Self {
            vertex_count,
            levels,
            counts,
        }
    }

    /// How many levels there are. `0` for a graph where nothing merged.
    #[must_use]
    pub const fn depth(&self) -> usize {
        self.levels.len()
    }

    /// The vertices the finest level partitions.
    #[must_use]
    pub const fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    /// Groups per level, finest first — the sequence a cut chooses out of.
    #[must_use]
    pub fn group_counts(&self) -> &[u32] {
        &self.counts
    }

    /// The levels themselves, for a caller that wants the raw walk — notably
    /// [`order_by_hierarchy`], whose parameter is this and not a [`Dendrogram`].
    #[must_use]
    pub fn levels(&self) -> &[Vec<u32>] {
        &self.levels
    }

    /// Give the levels up, so adopting a hierarchy and handing it on costs no
    /// copy of it.
    #[must_use]
    pub fn into_levels(self) -> Vec<Vec<u32>> {
        self.levels
    }

    /// One group id per **original vertex** at `level`, or `None` if there is no
    /// such level.
    ///
    /// The levels compose by lookup rather than recomputation, which is the
    /// reason the hierarchy is cheap to carry: this is `levels[0..=level]`
    /// applied in order, each one indexed by the last one's output.
    #[must_use]
    pub fn membership(&self, level: usize) -> Option<Vec<u32>> {
        let levels = self.levels.get(..=level)?;
        let mut label: Vec<u32> = (0..self.vertex_count).collect();
        for level in levels {
            for l in &mut label {
                *l = level[*l as usize];
            }
        }
        Some(label)
    }

    /// Which group of level `to` each group of level `from` belongs to — the
    /// parent column a holon row carries.
    ///
    /// `from` is strictly finer than `to`; `None` if either level is absent or
    /// they are the wrong way round. The walk is the same lookup composition as
    /// [`Self::membership`], started from `from`'s group ids instead of from the
    /// vertices.
    #[must_use]
    pub fn parents(&self, from: usize, to: usize) -> Option<Vec<u32>> {
        if from >= to {
            return None;
        }
        let levels = self.levels.get(from + 1..=to)?;
        let mut label: Vec<u32> = (0..*self.counts.get(from)?).collect();
        for level in levels {
            for l in &mut label {
                *l = level[*l as usize];
            }
        }
        Some(label)
    }

    /// **The declared cut**: the sequence of levels whose group counts fall by
    /// at least the pyramid's own branching factor at every step, stopping at
    /// the first rung that fits `chunk`.
    ///
    /// [`flatten_to_budget`] is the existing half of this — it walks the
    /// hierarchy and picks **one** level that fits a budget. This is the same
    /// walk choosing a **sequence**, and it exists because the levels Louvain
    /// emits are not a holarchy. Measured over com-DBLP by
    /// `crates/fossil-layout/examples/hierarchy_stats.rs` they contract 5.7×,
    /// 6.0×, 5.5×, then **2.5×** and **1.2×**: the last two steps change almost
    /// nothing on screen, and a reader ascending them takes a step that buys
    /// nothing. A cut states what a step must be worth and takes the levels that
    /// clear it.
    ///
    /// # Where the factor comes from, and why there is no `4` here
    ///
    /// The tile pyramid already fixes the octave —
    /// `crates/fossil-sinks/src/manifest.rs, VertexLevels` says a second
    /// spelling of that exponent is the bug its constant exists to prevent — so
    /// a rung's ceiling is `VertexLevels::rows_at(current, 1)`, which is level
    /// one of the tile pyramid over `current` rows. Same arithmetic, same
    /// integer, in every language that reads this corpus.
    ///
    /// # What it does NOT do
    ///
    /// It does not invent a level. A cut can only choose out of the partitions
    /// the dendrogram holds, so where Louvain's coarsening stalls the cut stops
    /// rather than publishing the stall: over com-DBLP it ends at 1,696 groups
    /// and the 690- and 595-group levels are dropped, because neither is a
    /// quarter of what stands above it. Whether the tree then gets a root is the
    /// caller's ruling, not this function's.
    ///
    /// An empty cut means there was nothing to choose: no hierarchy, a
    /// `chunk` of zero, a graph that already fits one chunk, or a first level
    /// too coarse to be a quarter of the graph.
    #[must_use]
    pub fn cut(&self, chunk: u64) -> Cut {
        let mut rungs: Vec<Rung> = Vec::new();
        let mut current = u64::from(self.vertex_count);
        let mut from = 0usize;
        while chunk > 0 && current > chunk {
            // `rows_at(.., 1)` is `ceil(current / 4)`: one level of the tile
            // pyramid, over this many rows instead of over the whole type.
            let ceiling = VertexLevels::rows_at(current, 1);
            // The FINEST level that fits under the ceiling, which is the one
            // that gives up least detail while still being worth a step. Levels
            // are scanned in index order from wherever the last rung landed, so
            // the answer is a pure function of the counts.
            let Some(level) =
                (from..self.counts.len()).find(|&l| u64::from(self.counts[l]) <= ceiling)
            else {
                break;
            };
            let groups = self.counts[level];
            rungs.push(Rung {
                level,
                groups,
                ceiling,
            });
            current = u64::from(groups);
            from = level + 1;
        }
        Cut {
            leaves: self.vertex_count,
            rungs,
        }
    }
}

/// A chosen sequence of levels, finest first.
///
/// The rungs are the levels a holon tree would publish; everything the
/// dendrogram holds between them is what the cut refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cut {
    /// What the finest rung contracts *from* — [`Dendrogram::vertex_count`], so
    /// the first contraction ratio is against the graph itself.
    leaves: u32,
    rungs: Vec<Rung>,
}

/// One level of a [`Cut`], and what it was chosen against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rung {
    level: usize,
    groups: u32,
    ceiling: u64,
}

impl Rung {
    /// Its index in the dendrogram — what [`Dendrogram::membership`] and
    /// [`Dendrogram::parents`] take.
    #[must_use]
    pub const fn level(self) -> usize {
        self.level
    }

    /// How many groups it has.
    #[must_use]
    pub const fn groups(self) -> u32 {
        self.groups
    }

    /// The most groups it was allowed to have: a quarter of the rung below,
    /// rounded up. A rung is usually well under its ceiling, because a cut
    /// chooses out of what a dendrogram happens to hold.
    #[must_use]
    pub const fn ceiling(self) -> u64 {
        self.ceiling
    }
}

impl Cut {
    /// **The branching factor, as the pyramid declares it.**
    ///
    /// `VertexLevels::stride(1)` and not a literal: the exponent lives in
    /// `crates/fossil-sinks/src/manifest.rs, VertexLevels` and a holon tree on
    /// the same octave inherits that arithmetic instead of inventing one.
    #[must_use]
    pub const fn declared_branching() -> u64 {
        VertexLevels::stride(1)
    }

    /// The rungs, finest first.
    #[must_use]
    pub fn rungs(&self) -> &[Rung] {
        &self.rungs
    }

    /// How many levels the cut publishes. At most `log₄(V / chunk)`, since every
    /// rung is at least a quarter-step and the walk stops at `chunk`.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rungs.len()
    }

    /// Whether the cut chose nothing at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rungs.is_empty()
    }

    /// What each rung contracts by, against the one below it — the first
    /// against the vertex count itself.
    ///
    /// Reporting only, which is why it is the one place in this type that
    /// touches `f64`: the guarantee is the integer one [`Rung::ceiling`]
    /// carries, and nothing branches on these.
    #[must_use]
    pub fn contractions(&self) -> Vec<f64> {
        let mut below = f64::from(self.leaves);
        self.rungs
            .iter()
            .map(|rung| {
                let ratio = below / f64::from(rung.groups.max(1));
                below = f64::from(rung.groups);
                ratio
            })
            .collect()
    }
}

/// How many groups a level names — `max + 1`, which for a level this module
/// produced is also its count of distinct ids, because [`densify`] relabels
/// them to `0..k`.
fn group_count(level: &[u32]) -> u32 {
    level.iter().copied().max().map_or(0, |m| m + 1)
}

#[cfg(test)]
mod component_tests {
    use super::*;

    #[test]
    fn wcc_two_disjoint_edges_two_clusters() {
        // 0—1 and 2—3 → {0,1}=0, {2,3}=1.
        let c = weakly_connected_components(4, &[(0, 1), (2, 3)]);
        assert_eq!(c, vec![0, 0, 1, 1]);
    }

    #[test]
    fn wcc_chain_is_one_cluster() {
        let c = weakly_connected_components(3, &[(0, 1), (1, 2)]);
        assert_eq!(c, vec![0, 0, 0]);
    }

    #[test]
    fn wcc_no_edges_each_isolated() {
        let c = weakly_connected_components(3, &[]);
        assert_eq!(c, vec![0, 1, 2]);
    }

    #[test]
    fn wcc_labels_are_dense_and_union_order_independent() {
        // Union in a different order must yield the same dense labelling.
        let a = weakly_connected_components(5, &[(3, 4), (0, 1)]);
        let b = weakly_connected_components(5, &[(0, 1), (3, 4)]);
        assert_eq!(a, b);
        // dense + gap-free: {0,1}=0, {2}=1, {3,4}=2.
        assert_eq!(a, vec![0, 0, 1, 2, 2]);
    }

    #[test]
    fn wcc_skips_out_of_range_endpoints() {
        // Endpoint 9 is out of range → ignored, no panic.
        let c = weakly_connected_components(2, &[(0, 9)]);
        assert_eq!(c, vec![0, 1]);
    }
}

#[cfg(test)]
mod hierarchy_tests {
    use super::*;

    /// Two cliques joined by a single edge is the canonical case: modularity
    /// must find the two cliques, where WCC finds one component.
    #[test]
    fn two_cliques_joined_by_a_bridge_become_two_communities() {
        let mut edges = Vec::new();
        for a in 0..4u32 {
            for b in (a + 1)..4 {
                edges.push((a, b));
            }
        }
        for a in 4..8u32 {
            for b in (a + 1)..8 {
                edges.push((a, b));
            }
        }
        edges.push((0, 4)); // the bridge

        // What we are measured against: reachability says "one".
        let wcc = weakly_connected_components(8, &edges);
        assert_eq!(wcc.iter().copied().max(), Some(0), "the graph is connected");

        let levels = community_hierarchy(8, &edges);
        assert!(!levels.is_empty(), "a bridged pair of cliques must split");
        let first = &levels[0];
        assert_eq!(first.len(), 8);
        for v in 1..4 {
            assert_eq!(first[v], first[0], "clique A stays together");
        }
        for v in 5..8 {
            assert_eq!(first[v], first[4], "clique B stays together");
        }
        assert_ne!(first[0], first[4], "the two cliques are not one community");
    }

    /// Every level's ids are dense and every level indexes the one below, or the
    /// pyramid cannot be walked.
    #[test]
    fn levels_are_dense_and_stack() {
        let mut edges = Vec::new();
        // Four cliques of four, chained by single bridges.
        for c in 0..4u32 {
            let base = c * 4;
            for a in 0..4u32 {
                for b in (a + 1)..4 {
                    edges.push((base + a, base + b));
                }
            }
            if c > 0 {
                edges.push((base, base - 4));
            }
        }
        let levels = community_hierarchy(16, &edges);
        assert!(!levels.is_empty());
        assert_eq!(levels[0].len(), 16, "level 0 is indexed by vertex");
        for (l, level) in levels.iter().enumerate() {
            let count = level.iter().copied().max().map_or(0, |m| m + 1) as usize;
            let mut seen = vec![false; count];
            for &c in level {
                seen[c as usize] = true;
            }
            assert!(seen.iter().all(|s| *s), "level {l} ids must be dense");
            if let Some(next) = levels.get(l + 1) {
                assert_eq!(
                    next.len(),
                    count,
                    "level {} indexes level {l}'s output",
                    l + 1
                );
            }
        }
    }

    /// Determinism is a promise this module makes everywhere else, so it is
    /// tested rather than assumed.
    #[test]
    fn the_same_graph_gives_the_same_hierarchy() {
        let edges: Vec<(u32, u32)> = (0..40u32).map(|i| (i % 10, (i * 7) % 10)).collect();
        assert_eq!(
            community_hierarchy(10, &edges),
            community_hierarchy(10, &edges)
        );
    }

    /// The budget picks a level, and picking is the whole job: a hierarchy has
    /// no single "the" partition.
    #[test]
    fn flatten_takes_the_finest_level_that_fits() {
        // Six vertices → three pairs → one community. Levels are stated rather
        // than computed so the test is about the choosing, not about Louvain.
        let levels = vec![vec![0, 0, 1, 1, 2, 2], vec![0, 0, 0]];

        assert_eq!(
            flatten_to_budget(&levels, 6, 3),
            (vec![0, 0, 1, 1, 2, 2], Some(0)),
            "three communities fit a budget of three, so the finest level wins",
        );
        assert_eq!(
            flatten_to_budget(&levels, 6, 2),
            (vec![0, 0, 0, 0, 0, 0], Some(1)),
            "over budget, the walk composes up a level",
        );
        assert_eq!(
            flatten_to_budget(&levels, 6, 1),
            (vec![0, 0, 0, 0, 0, 0], Some(1)),
            "a budget nothing satisfies still stops at the coarsest level there is",
        );
    }

    /// A graph with no edges has no hierarchy, and then every vertex is its own
    /// community — which is the truth about that graph, not a fallback.
    #[test]
    fn flatten_without_a_hierarchy_isolates_every_vertex() {
        assert_eq!(flatten_to_budget(&[], 4, 2_048), (vec![0, 1, 2, 3], None));
    }

    /// The number a community wears decides where it is drawn, so siblings have
    /// to be consecutive — otherwise two communities with thousands of edges
    /// between them land on opposite sides of the picture as often as not.
    #[test]
    fn ordering_puts_siblings_next_to_each_other() {
        // Six communities under two parents, interleaved on purpose: the odd
        // ones belong to parent 0 and the even ones to parent 1, which is the
        // arrangement id order gets wrong.
        let levels = vec![Vec::new(), vec![1, 0, 1, 0, 1, 0]];
        let mut membership: Vec<u32> = vec![0, 1, 2, 3, 4, 5];
        order_by_hierarchy(&levels, 0, &mut membership);

        let parent = |c: u32| levels[1][c as usize];
        let mut by_rank: Vec<(u32, u32)> = (0..6).map(|c| (membership[c as usize], c)).collect();
        by_rank.sort_unstable();
        let families: Vec<u32> = by_rank.iter().map(|&(_, c)| parent(c)).collect();

        // Each family occupies one contiguous run, so the sequence changes hands
        // exactly once.
        let switches = families.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(
            switches, 1,
            "families must not be interleaved: {families:?}"
        );
    }

    /// One orientation of `edges` as the writer emits it — keyed, sorted, then
    /// pushed through the very builder `walk_orientation` feeds from Arrow.
    fn side(
        n: u32,
        edges: &[(u32, u32)],
        key: impl Fn(&(u32, u32)) -> (u32, u32),
        self_loops: &mut [f64],
    ) -> Csr {
        let mut rows: Vec<(u32, u32)> = edges.iter().map(key).collect();
        rows.sort_unstable();
        let (keys, values): (Vec<u32>, Vec<u32>) = rows.into_iter().unzip();
        let mut builder = CsrBuilder::new(n as usize, keys.len());
        assert!(builder.push(&keys, &values, self_loops));
        builder.finish()
    }

    /// The claim the CSR read path rests on: reading the two
    /// orientations the artefact already stores builds the **same graph** as
    /// handing the same edges over as an unordered bag. Exactly the same, not
    /// nearly — modularity is defined over sums, every weight at level 0 is one,
    /// and a sum of ones is exact in `f64`, so the two hierarchies are compared
    /// whole rather than by some tolerance.
    #[test]
    fn the_orientations_on_disk_and_the_bag_are_one_graph() {
        const N: u32 = 24;
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for c in 0..4u32 {
            let base = c * 6;
            for a in 0..6u32 {
                for b in (a + 1)..6 {
                    edges.push((base + a, base + b));
                }
            }
            if c > 0 {
                edges.push((base, base - 6));
            }
        }
        edges.push((7, 7)); // a self-loop, which is a row in *both* files

        let mut self_loops = vec![0.0f64; N as usize];
        let sides = vec![
            side(N, &edges, |&(a, b)| (a, b), &mut self_loops),
            side(N, &edges, |&(a, b)| (b, a), &mut self_loops),
        ];
        for count in &mut self_loops {
            *count /= 2.0;
        }

        assert_eq!(
            community_hierarchy(N, &edges),
            hierarchy(Weighted::finish(sides, self_loops)),
            "the CSR on disk and the bag in memory are the same graph",
        );
    }

    /// A file that is not in the order it declares does not fail, it builds a
    /// different graph — so the builder refuses it rather than believing it.
    #[test]
    fn a_key_that_goes_backwards_is_refused() {
        let mut self_loops = vec![0.0f64; 4];
        let mut builder = CsrBuilder::new(4, 3);
        assert!(builder.push(&[0, 2], &[1, 3], &mut self_loops));
        assert!(!builder.push(&[1], &[0], &mut self_loops));
    }

    /// Vertices with no edges are the common case at both ends of the range, and
    /// the file says nothing about them. Their offsets still have to be written
    /// — the ones it skips over and the tail it stops before — or one vertex's
    /// neighbour list reads off into another's.
    #[test]
    fn the_builder_fills_the_vertices_the_file_never_mentions() {
        let mut self_loops = vec![0.0f64; 5];
        let mut builder = CsrBuilder::new(5, 2);
        assert!(builder.push(&[1, 1], &[0, 3], &mut self_loops));
        let csr = builder.finish();
        assert_eq!(csr.offsets, vec![0, 0, 2, 2, 2, 2]);
        assert_eq!(csr.targets, vec![0, 3]);
    }

    /// Degenerate shapes must not panic or invent levels.
    #[test]
    fn empty_and_edgeless_graphs_have_no_hierarchy() {
        assert!(community_hierarchy(0, &[]).is_empty());
        assert!(
            community_hierarchy(5, &[]).is_empty(),
            "with no edges nothing merges, so there is no level to record"
        );
    }
}

#[cfg(test)]
mod dendrogram_tests {
    use super::*;

    /// A level mapping `n` nodes onto `k` groups, contiguously and densely —
    /// stated rather than computed, so these tests are about the **choosing**
    /// and not about Louvain.
    fn level(n: u32, k: u32) -> Vec<u32> {
        // In `u64`: the products here are a vertex count times a group count,
        // which at com-DBLP's scale is well past `u32`.
        let (n64, k64) = (u64::from(n), u64::from(k));
        (0..n)
            .map(|i| ((u64::from(i) * k64) / n64) as u32)
            .collect()
    }

    /// The counts a dendrogram of these widths has, which is what a cut chooses
    /// out of.
    fn of_counts(counts: &[u32], vertex_count: u32) -> Dendrogram {
        let mut levels = Vec::with_capacity(counts.len());
        let mut below = vertex_count;
        for &k in counts {
            levels.push(level(below, k));
            below = k;
        }
        Dendrogram::new(vertex_count, levels).expect("stated levels stack")
    }

    #[test]
    fn a_stated_hierarchy_reports_its_own_widths() {
        let d = of_counts(&[40, 16, 4], 64);
        assert_eq!(d.depth(), 3);
        assert_eq!(d.vertex_count(), 64);
        assert_eq!(d.group_counts(), &[40, 16, 4]);
    }

    #[test]
    fn levels_that_do_not_stack_are_refused_rather_than_indexed() {
        assert_eq!(
            Dendrogram::new(6, vec![vec![0, 0, 1, 1, 2, 2], vec![0, 0]]),
            Err(DendrogramError::Unstacked {
                level: 1,
                found: 2,
                expected: 3,
            }),
            "level 1 is indexed by level 0's three groups, not by two",
        );
        assert_eq!(
            Dendrogram::new(5, vec![vec![0, 0, 1]]),
            Err(DendrogramError::Unstacked {
                level: 0,
                found: 3,
                expected: 5,
            }),
            "the finest level is indexed by the vertices",
        );
    }

    #[test]
    fn membership_and_parents_are_the_same_lookup_walk() {
        // Six vertices → three pairs → one group.
        let d =
            Dendrogram::new(6, vec![vec![0, 0, 1, 1, 2, 2], vec![0, 0, 0]]).expect("levels stack");

        assert_eq!(d.membership(0), Some(vec![0, 0, 1, 1, 2, 2]));
        assert_eq!(d.membership(1), Some(vec![0, 0, 0, 0, 0, 0]));
        assert_eq!(d.membership(2), None, "there is no third level");

        assert_eq!(
            d.parents(0, 1),
            Some(vec![0, 0, 0]),
            "three groups, one parent"
        );
        assert_eq!(d.parents(1, 1), None, "a level is not its own parent");
        assert_eq!(d.parents(1, 0), None, "parents are coarser, never finer");
        assert_eq!(d.parents(0, 2), None, "there is no third level");
    }

    /// The whole of B2.2: a rung is worth taking only if it contracts by the
    /// factor the tile pyramid declares.
    #[test]
    fn every_rung_contracts_by_at_least_the_declared_factor() {
        let d = of_counts(&[40, 16, 15, 4], 64);
        let cut = d.cut(1);

        // 64 → ceiling 16 skips the 40, which is not a quarter of the graph;
        // 16 → ceiling 4 skips the 15, which is not a quarter of the 16.
        let levels: Vec<usize> = cut.rungs().iter().map(|r| r.level()).collect();
        assert_eq!(levels, vec![1, 3], "the levels that buy a step: {cut:?}");
        assert_eq!(
            cut.rungs().iter().map(|r| r.groups()).collect::<Vec<_>>(),
            vec![16, 4],
        );

        let mut below = u64::from(d.vertex_count());
        for rung in cut.rungs() {
            assert!(
                u64::from(rung.groups()) * Cut::declared_branching() <= below,
                "rung {rung:?} does not contract by {}× from {below}",
                Cut::declared_branching(),
            );
            below = u64::from(rung.groups());
        }
    }

    /// Where the coarsening stalls, the cut stops rather than publishing the
    /// stall — com-DBLP's 690 and 595 are the measured case.
    #[test]
    fn a_dendrogram_that_stalls_is_cut_short_of_its_top() {
        let d = of_counts(&[55_712, 9_248, 1_696, 690, 595], 317_080);
        let cut = d.cut(1);

        assert_eq!(
            cut.rungs().iter().map(|r| r.groups()).collect::<Vec<_>>(),
            vec![55_712, 9_248, 1_696],
            "690 is not a quarter of 1,696 and 595 is not a quarter of 690",
        );
        assert!(
            cut.contractions().iter().all(|&r| r >= 4.0),
            "measured contractions: {:?}",
            cut.contractions(),
        );
    }

    #[test]
    fn the_walk_stops_at_the_chunk_that_already_fits() {
        let d = of_counts(&[40, 16, 4, 1], 64);
        assert_eq!(d.cut(16).len(), 1, "16 groups already fit a chunk of 16");
        assert_eq!(d.cut(4).len(), 2);
        assert_eq!(d.cut(1).len(), 3);
    }

    #[test]
    fn nothing_to_choose_from_cuts_to_nothing() {
        let d = of_counts(&[40, 16, 4], 64);
        assert!(d.cut(0).is_empty(), "a chunk of zero is not a budget");
        assert!(
            d.cut(64).is_empty(),
            "a graph that fits one chunk has no tree"
        );
        assert!(
            Dendrogram::of(5, &[]).cut(1).is_empty(),
            "no hierarchy, nothing to cut",
        );
        assert!(
            of_counts(&[40], 64).cut(1).is_empty(),
            "a first level that is not a quarter of the graph is not a rung",
        );
    }

    /// Determinism is the property `/docs/design/holons` puts first, and a cut
    /// must not be where it is lost: every walk above is over `Vec`s in index
    /// order, so the same levels give the same rungs.
    #[test]
    fn the_same_hierarchy_gives_the_same_cut() {
        let edges: Vec<(u32, u32)> = (0..400u32).map(|i| (i % 100, (i * 7) % 100)).collect();
        let a = Dendrogram::of(100, &edges);
        let b = Dendrogram::of(100, &edges);
        assert_eq!(a, b);
        assert_eq!(a.cut(4), b.cut(4));
        assert_eq!(a.membership(0), b.membership(0));
    }

    /// The dendrogram is the levels and nothing else: adopting what
    /// [`community_hierarchy`] returned and giving it back is the identity.
    #[test]
    fn adopting_the_hierarchy_changes_nothing_about_it() {
        let mut edges = Vec::new();
        for c in 0..4u32 {
            let base = c * 4;
            for a in 0..4u32 {
                for b in (a + 1)..4 {
                    edges.push((base + a, base + b));
                }
            }
            if c > 0 {
                edges.push((base, base - 4));
            }
        }
        let levels = community_hierarchy(16, &edges);
        assert_eq!(
            Dendrogram::new(16, levels.clone())
                .expect("Louvain's levels stack")
                .into_levels(),
            levels,
        );
        assert_eq!(Dendrogram::of(16, &edges).levels(), levels);
    }
}
