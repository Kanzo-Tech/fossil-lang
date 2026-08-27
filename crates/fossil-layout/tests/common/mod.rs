//! Fixture construction shared by the two budget suites.
//!
//! `budget.rs` asserts the refusal's contract; `budget_bound.rs` measures a
//! resident set and therefore has to be the only test in its process. They need
//! the same staged corpus, and a corpus generator copied into two files is two
//! generators the moment one of them is edited.
//!
//! `dead_code` is allowed because this module is compiled into BOTH targets and
//! neither uses all of it — `budget.rs` compares whole trees and reads `root`,
//! `budget_bound.rs` samples a resident set and does not. The alternative is a
//! `cfg` per helper, which is a worse way to say the same thing.
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
use parquet::arrow::ArrowWriter;

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

fn url(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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

/// The vertex schema a staged corpus has: the four columns the pass replaces,
/// plus a subject IRI wide enough that the vertex file is a real term in the
/// budget rather than four integer columns.
fn vertex_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]))
}

fn write_vertices(at: &Path, rows: u32) {
    let schema = vertex_schema();
    let file = fs::File::create(at).expect("create the vertex parquet");
    let mut writer = ArrowWriter::try_new(file, Arc::clone(&schema), None).expect("a writer");
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
    writer
        .write(&RecordBatch::try_new(schema, columns).expect("a batch"))
        .expect("write the vertices");
    writer.close().expect("close the vertex parquet");
}

/// One orientation, sorted by the endpoint it is ordered on — which is what the
/// pass reads it as, and what `LayoutError::Disordered` refuses it for otherwise.
fn write_adjacency(at: &Path, edges: &[(u32, u32)], by: Endpoint) {
    let mut rows: Vec<(u32, u32)> = edges.to_vec();
    match by {
        Endpoint::Src => rows.sort_unstable_by_key(|&(s, d)| (s, d)),
        Endpoint::Dst => rows.sort_unstable_by_key(|&(s, d)| (d, s)),
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
    ]));
    let file = fs::File::create(at).expect("create the adjacency parquet");
    let mut writer = ArrowWriter::try_new(file, Arc::clone(&schema), None).expect("a writer");
    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from(
            rows.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|&(_, d)| d).collect::<Vec<_>>(),
        )),
    ];
    writer
        .write(&RecordBatch::try_new(schema, columns).expect("a batch"))
        .expect("write the adjacency");
    writer.close().expect("close the adjacency parquet");
}

/// A whole staged corpus in `root`, plus the targets that name it.
pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) targets: Vec<VertexLayoutTarget>,
    pub(crate) adjacencies: Vec<AdjacencyTarget>,
}

pub(crate) fn fixture(root: PathBuf, rows: u32, mean_degree: u32) -> Fixture {
    fs::create_dir_all(root.join("chunks")).expect("create the tile directory");
    let vertices = root.join("Node.parquet");
    let by_source = root.join("by_source.parquet");
    let by_target = root.join("by_target.parquet");

    write_vertices(&vertices, rows);
    let edges = planted(rows, mean_degree);
    write_adjacency(&by_source, &edges, Endpoint::Src);
    write_adjacency(&by_target, &edges, Endpoint::Dst);

    let targets = vec![VertexLayoutTarget {
        type_name: "Node".to_string(),
        vertex_parquet: url(&vertices),
        chunk_prefix: format!("{}{}", url(&root.join("chunks")), std::path::MAIN_SEPARATOR),
        chunk_size: 4_096,
        self_edge_csr: vec![url(&by_source)],
    }];
    let adjacencies = vec![
        AdjacencyTarget {
            parquet: url(&by_source),
            src_type: "Node".to_string(),
            dst_type: "Node".to_string(),
            ordered_by: Endpoint::Src,
        },
        AdjacencyTarget {
            parquet: url(&by_target),
            src_type: "Node".to_string(),
            dst_type: "Node".to_string(),
            ordered_by: Endpoint::Dst,
        },
    ];
    Fixture {
        root,
        targets,
        adjacencies,
    }
}
