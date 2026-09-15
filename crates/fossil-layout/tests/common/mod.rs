//! Fixture construction shared by the two budget suites, and [`tree`] — what a
//! suite reads a written corpus as when what it has to say about one is that it
//! is the same bytes as another.
//!
//! `budget.rs` asserts the refusal's contract; `budget_bound.rs` measures a
//! resident set and therefore has to be the only test in its process. They need
//! the same input corpus, and a corpus generator copied into two files is two
//! generators the moment one of them is edited.
//!
//! [`tree`] came here by that same argument one artefact later. `budget.rs`
//! compares a bounded run against an unbounded one and `cells.rs` compares two
//! *processes*: two suites asking whether two corpora are the same bytes, and a
//! directory walk copied into both is the walk that stops agreeing about what a
//! corpus contains the day one of them learns about a new directory.
//!
//! `dead_code` is allowed because this module is compiled into every target that
//! takes it and none of them uses all of it — `budget.rs` compares whole trees
//! and reads `root`, `budget_bound.rs` samples a resident set and does not, and
//! `cells.rs` plants its own fixture and takes the walk alone. The alternative
//! is a `cfg` per helper, which is a worse way to say the same thing.
#![allow(dead_code)]
// `pub(crate)` here is what `unreachable_pub` asks for and what `redundant_pub_crate`
// objects to — the two lints disagree about a private module in a test target, and
// there is no spelling that satisfies both. The visibility that matters is the one
// the lints agree is correct: nothing outside this test binary can reach it either way.
#![allow(clippy::redundant_pub_crate)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use fossil_layout::layout::{AdjacencyTarget, Endpoint, VertexLayoutTarget};

/// A fresh, empty directory under `std::env::temp_dir()`.
///
/// The `<pid>` is load-bearing for the reason `layout_renumber.rs` spells out at
/// length: the directory is wiped before it is seeded, so a path keyed only on
/// the test name is shared by every process on the machine running this binary.
pub(crate) fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("fossil_budget_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create test dir");
    path
}

/// Every file under `root`, as `(relative path, bytes)`, sorted — so two corpora
/// compare as one value and a difference names the file it is in.
pub(crate) fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(at: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(at)
            .expect("read the corpus directory")
            .map(|e| e.expect("a directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .expect("a path under the root")
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, fs::read(&path).expect("read a corpus file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn url(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A prefix the pass can write into: a URL with the platform's separator on the
/// end, which is what both filesystems require of one.
fn prefix(path: &Path) -> String {
    format!("{}{}", url(path), std::path::MAIN_SEPARATOR)
}

/// A planted-partition graph, the same shape `examples/enrich_memory` measures
/// on: most edges inside a block of a thousand, a tenth of them anywhere. Louvain
/// on a uniform random graph merges nothing and the hierarchy is one level deep,
/// which would exercise the easy case and report it as the cost.
fn planted(n: u32, mean_degree: u32) -> Vec<(u32, u32)> {
    let block = 1_000.min(n.max(1));
    let mut edges = Vec::with_capacity((n as usize * mean_degree as usize) / 2);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for v in 0..n {
        let home = v / block;
        for _ in 0..(mean_degree / 2) {
            let u = if next() % 10 == 0 {
                u32::try_from(next() % u64::from(n)).unwrap_or(0)
            } else {
                let base = home * block;
                let span = block.min(n - base);
                base + u32::try_from(next() % u64::from(span)).unwrap_or(0)
            };
            if u != v {
                edges.push((v, u));
            }
        }
    }
    edges
}

/// The vertex schema the writer produces: the four columns the pass replaces,
/// plus a subject IRI wide enough that the payload is a real term in the budget
/// rather than four integer columns.
fn vertex_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]))
}

/// The vertices as the executor hands them over: one batch, the four columns
/// the pass replaces, and a subject IRI wide enough that the payload is a real
/// term in the budget.
fn vertices(rows: u32) -> Vec<RecordBatch> {
    let schema = vertex_schema();
    let ids: Vec<u32> = (0..rows).collect();
    let subjects: Vec<String> = (0..rows)
        .map(|i| format!("https://example.org/dataset/entity/{i:012}"))
        .collect();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from(ids)),
        Arc::new(StringArray::from(subjects)),
        Arc::new(Float32Array::from(vec![0.0f32; rows as usize])),
        Arc::new(Float32Array::from(vec![0.0f32; rows as usize])),
        Arc::new(UInt32Array::from(vec![0u32; rows as usize])),
    ];
    vec![RecordBatch::try_new(schema, columns).expect("a batch")]
}

/// One orientation, sorted by the endpoint it is ordered on — which is what the
/// pass reads it as, and what `LayoutError::Disordered` refuses it for otherwise.
fn adjacency(edges: &[(u32, u32)], by: Endpoint) -> Vec<RecordBatch> {
    let mut rows: Vec<(u32, u32)> = edges.to_vec();
    match by {
        Endpoint::Src => rows.sort_unstable_by_key(|&(s, d)| (s, d)),
        Endpoint::Dst => rows.sort_unstable_by_key(|&(s, d)| (d, s)),
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from(
            rows.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|&(_, d)| d).collect::<Vec<_>>(),
        )),
    ];
    vec![RecordBatch::try_new(schema, columns).expect("a batch")]
}

/// A corpus's worth of INPUT — the batches an `execute_graph` would have
/// returned — plus the directory the pass is to write into.
///
/// It holds the batches and hands out the targets that borrow them, rather than
/// holding both: a struct carrying a `VertexLayoutTarget<'_>` beside the
/// `Vec<RecordBatch>` it points at is self-referential and does not compile. The
/// two methods are also the honest shape, because the targets are cheap and the
/// batches are the corpus.
///
/// **Nothing here is on disk any more.** This used to write a vertex Parquet and
/// two adjacency Parquet under `root` and point the targets at them, because the
/// pass opened files; `root` now starts empty and only ever holds output.
pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    /// Rows per tile, which `levels.rs` turns down so that four thousand rows
    /// span a five-level pyramid instead of four million.
    pub(crate) chunk_size: u64,
    /// The base of the cell pyramid — **the default, so the guard bounds the
    /// pass a real run runs.**
    ///
    /// It was `None` while `estimated_peak_bytes` had no term for a pyramid, on
    /// the grounds that a fixture writing one would measure an extrapolation
    /// rather than the bound. The term is there now: `VERTEX_ARRAY_BYTES`
    /// carries 3 bytes per vertex at this base, derived from `Rung`'s columns
    /// and the quarter series over them.
    ///
    /// **What the measurement behind it found is that the peak does not move**,
    /// because it is set by `community_hierarchy` and the pyramid runs inside
    /// pages `flatten + order + place` has already released. So switching this
    /// on is not expected to change what `budget_bound.rs` reads — and it being
    /// on is what makes that a prediction this suite can falsify rather than a
    /// paragraph.
    pub(crate) vertices_per_cell: Option<u64>,
    vertices: Vec<RecordBatch>,
    by_source: Vec<RecordBatch>,
    by_target: Vec<RecordBatch>,
}

impl Fixture {
    /// The one vertex type, writing its tiles under `root/chunks/`.
    pub(crate) fn targets(&self) -> Vec<VertexLayoutTarget<'_>> {
        vec![VertexLayoutTarget {
            type_name: "Node".to_string(),
            batches: &self.vertices,
            chunk_prefix: prefix(&self.root.join("chunks")),
            chunk_size: self.chunk_size,
            vertices_per_cell: self.vertices_per_cell,
        }]
    }

    /// Both orientations of the one self-relation, `Node_edge_Node`.
    pub(crate) fn adjacencies(&self) -> Vec<AdjacencyTarget<'_>> {
        [
            ("by_source", Endpoint::Src, &self.by_source),
            ("by_target", Endpoint::Dst, &self.by_target),
        ]
        .into_iter()
        .map(|(dir, ordered_by, batches)| AdjacencyTarget {
            src_type: "Node".to_string(),
            label: "edge".to_string(),
            dst_type: "Node".to_string(),
            ordered_by,
            batches,
            tile_prefix: prefix(&self.root.join(dir)),
            levels_prefix: prefix(&self.root),
        })
        .collect()
    }
}

pub(crate) fn fixture(root: PathBuf, rows: u32, mean_degree: u32) -> Fixture {
    fs::create_dir_all(root.join("chunks")).expect("create the tile directory");
    let edges = planted(rows, mean_degree);
    Fixture {
        chunk_size: 4_096,
        vertices_per_cell: Some(fossil_sinks::manifest::DEFAULT_VERTICES_PER_CELL),
        vertices: vertices(rows),
        by_source: adjacency(&edges, Endpoint::Src),
        by_target: adjacency(&edges, Endpoint::Dst),
        root,
    }
}
