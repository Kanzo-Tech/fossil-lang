//! Fixture construction shared by the two budget suites.
//!
//! `budget.rs` asserts the refusal's contract; `budget_bound.rs` measures a
//! resident set and therefore has to be the only test in its process. They need
//! the same input graph, and a graph generator copied into two files is two
//! generators the moment one of them is edited.
// `pub(crate)` here is what `unreachable_pub` asks for and what `redundant_pub_crate`
// objects to — the two lints disagree about a private module in a test target, and
// there is no spelling that satisfies both. The visibility that matters is the one
// the lints agree is correct: nothing outside this test binary can reach it either way.
#![allow(clippy::redundant_pub_crate)]

use std::sync::Arc;

use arrow::array::{ArrayRef, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use fossil_layout::layout::{Relation, VertexType};

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

/// The vertex schema the executor produces, with a subject IRI wide enough that
/// the payload is a real term in the budget rather than four integer columns.
fn vertex_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]))
}

/// The vertices as the executor hands them over: one batch, `dense_id` its row
/// number, and a subject IRI wide enough that the payload is a real term in the
/// budget.
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

/// The relation, sorted by `(src, dst)` as the executor emits it.
fn relation(edges: &[(u32, u32)]) -> Vec<RecordBatch> {
    let mut rows: Vec<(u32, u32)> = edges.to_vec();
    rows.sort_unstable();
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

/// A graph's worth of INPUT — the batches an `execute_graph` would have
/// returned. It holds the batches and hands out the views that borrow them.
pub(crate) struct Fixture {
    vertices: Vec<RecordBatch>,
    edges: Vec<RecordBatch>,
}

impl Fixture {
    /// The one vertex type.
    pub(crate) fn types(&self) -> Vec<VertexType<'_>> {
        vec![VertexType {
            name: "Node",
            batches: &self.vertices,
        }]
    }

    /// The one self-relation, `Node_edge_Node`.
    pub(crate) fn relations(&self) -> Vec<Relation<'_>> {
        vec![Relation {
            name: "Node_edge_Node",
            source: 0,
            destination: 0,
            batches: &self.edges,
        }]
    }
}

pub(crate) fn fixture(rows: u32, mean_degree: u32) -> Fixture {
    let edges = planted(rows, mean_degree);
    Fixture {
        vertices: vertices(rows),
        edges: relation(&edges),
    }
}
