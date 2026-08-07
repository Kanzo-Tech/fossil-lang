//! Build a [`GraphArData`] from **literal** vertex/edge data — rows the host
//! already holds in memory, not a query over a source. This is the catalog
//! path: a DCAT-AP graph is a handful of known rows (one per dataset /
//! distribution / field), so it needs no executor — just the same dense-id +
//! CSR/CSC + graph-schema assembly the program path produces, fed from literals.
//!
//! Pure Arrow (no DataFusion plan, no DuckDB): the universal substrate's
//! materializer reused for a non-program graph. Mirrors `execute_graph`'s output
//! shape exactly (the W0b columns, deterministic dense ids by subject), so the
//! same `write_to_dir` / `manifests` / `run_status` consume it unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, BooleanArray, Float32Array, Float64Array, Int64Array, StringArray, UInt32Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use fossil_graph_schema::{
    Cardinality, EdgeType, GraphSchema, NodeType, Primitive, Property as NodeProp,
};

use crate::{EdgeTable, GraphArData, VertexTable};

/// One vertex type's literal rows. `row[0]` is the subject IRI; `row[i + 1]` is
/// the value of `properties[i]` (an empty string is read as null).
#[derive(Debug, Clone)]
pub struct LiteralVertex {
    pub label: String,
    pub rdf_type: Option<String>,
    pub properties: Vec<NodeProp>,
    pub rows: Vec<Vec<String>>,
}

/// One edge type's literal `(src_iri, dst_iri)` endpoint pairs.
#[derive(Debug, Clone)]
pub struct LiteralEdge {
    pub label: String,
    pub rdf_uri: Option<String>,
    pub src_type: String,
    pub dst_type: String,
    pub single_valued: bool,
    pub pairs: Vec<(String, String)>,
}

/// Assemble a [`GraphArData`] from literal vertices + edges: sort each vertex
/// type by subject for a deterministic `dense_id`, resolve every edge endpoint
/// IRI to its dense id (dangling endpoints drop, like the program path), and
/// order the pairs into CSR (`by_source`) + CSC (`by_target`). The canonical
/// [`GraphSchema`] is built alongside; manifests/`RunStatus` derive from it.
#[must_use]
pub fn from_literal_graph(vertices: Vec<LiteralVertex>, edges: Vec<LiteralEdge>) -> GraphArData {
    let mut vertex_tables = Vec::with_capacity(vertices.len());
    let mut nodes = Vec::with_capacity(vertices.len());
    // subject IRI → dense id, per vertex type — the edge phase joins against it.
    let mut dense_of: HashMap<String, HashMap<String, u32>> = HashMap::new();

    for v in vertices {
        let mut order: Vec<usize> = (0..v.rows.len()).collect();
        order.sort_by(|&a, &b| v.rows[a][0].cmp(&v.rows[b][0]));

        let mut subject_to_dense = HashMap::with_capacity(order.len());
        let subjects: Vec<&str> = order
            .iter()
            .enumerate()
            .map(|(dense, &row)| {
                let iri = v.rows[row][0].as_str();
                subject_to_dense.insert(iri.to_string(), u32::try_from(dense).unwrap_or(u32::MAX));
                iri
            })
            .collect();

        let n = order.len();
        let mut fields = vec![
            Field::new("dense_id", DataType::UInt32, false),
            Field::new("subject", DataType::Utf8, false),
        ];
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(UInt32Array::from_iter_values(
                0..u32::try_from(n).unwrap_or(u32::MAX),
            )),
            Arc::new(subjects.iter().map(|s| Some(*s)).collect::<StringArray>()),
        ];

        for (i, prop) in v.properties.iter().enumerate() {
            // The literal value of this property for each row, in dense order.
            let vals = order.iter().map(|&row| {
                v.rows[row]
                    .get(i + 1)
                    .map(String::as_str)
                    .filter(|s| !s.is_empty())
            });
            let (arr, dt) = literal_column(prop.datatype, vals);
            fields.push(Field::new(&prop.name, dt, true));
            columns.push(arr);
        }

        fields.push(Field::new("x", DataType::Float32, false));
        fields.push(Field::new("y", DataType::Float32, false));
        fields.push(Field::new("cluster_id", DataType::UInt32, false));
        columns.push(Arc::new(Float32Array::from(vec![0.0_f32; n])));
        columns.push(Arc::new(Float32Array::from(vec![0.0_f32; n])));
        columns.push(Arc::new(UInt32Array::from(vec![0_u32; n])));

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .expect("literal vertex columns are equal length");
        dense_of.insert(v.label.clone(), subject_to_dense);
        nodes.push(NodeType {
            label: v.label.clone(),
            iri: v.rdf_type,
            properties: v.properties,
        });
        vertex_tables.push(VertexTable {
            label: v.label,
            batches: vec![batch],
        });
    }

    let mut edge_tables = Vec::with_capacity(edges.len());
    let mut edge_types = Vec::with_capacity(edges.len());
    for e in edges {
        let src_map = dense_of.get(&e.src_type);
        let dst_map = dense_of.get(&e.dst_type);
        let mut resolved: Vec<(u32, u32)> = e
            .pairs
            .iter()
            .filter_map(|(s, d)| Some((*src_map?.get(s)?, *dst_map?.get(d)?)))
            .collect();

        resolved.sort_unstable(); // CSR: by (src, dst)
        let by_source = pair_batch(&resolved);
        resolved.sort_unstable_by_key(|&(s, d)| (d, s)); // CSC: by (dst, src)
        let by_target = pair_batch(&resolved);

        edge_types.push(EdgeType {
            label: e.label.clone(),
            iri: e.rdf_uri,
            source: e.src_type.clone(),
            destination: e.dst_type.clone(),
            cardinality: if e.single_valued {
                Cardinality::Single
            } else {
                Cardinality::Multi
            },
        });
        edge_tables.push(EdgeTable {
            label: e.label,
            src_type: e.src_type,
            dst_type: e.dst_type,
            by_source,
            by_target,
        });
    }

    GraphArData {
        schema: GraphSchema {
            nodes,
            edges: edge_types,
        },
        vertices: vertex_tables,
        edges: edge_tables,
    }
}

/// Build the typed Arrow array for a literal property column, honouring the
/// schema datatype so the Parquet type matches the manifest spelling (e.g.
/// `Integer` → `Int64`). Unparseable / absent values become null. Non-scalar
/// datatypes (dates, IRIs) keep their lexical string form.
fn literal_column<'a>(
    datatype: Primitive,
    values: impl Iterator<Item = Option<&'a str>>,
) -> (ArrayRef, DataType) {
    match datatype {
        Primitive::Integer => (
            Arc::new(
                values
                    .map(|v| v.and_then(|s| s.parse::<i64>().ok()))
                    .collect::<Int64Array>(),
            ),
            DataType::Int64,
        ),
        Primitive::Float => (
            Arc::new(
                values
                    .map(|v| v.and_then(|s| s.parse::<f64>().ok()))
                    .collect::<Float64Array>(),
            ),
            DataType::Float64,
        ),
        Primitive::Bool => (
            Arc::new(
                values
                    .map(|v| v.and_then(|s| s.parse::<bool>().ok()))
                    .collect::<BooleanArray>(),
            ),
            DataType::Boolean,
        ),
        _ => (Arc::new(values.collect::<StringArray>()), DataType::Utf8),
    }
}

/// A `(src_dense, dst_dense)` adjacency `RecordBatch` (the two `u32` columns the
/// CSR/CSC Parquet carries), already in the caller's chosen order.
fn pair_batch(pairs: &[(u32, u32)]) -> Vec<RecordBatch> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
    ]));
    let src = UInt32Array::from_iter_values(pairs.iter().map(|&(s, _)| s));
    let dst = UInt32Array::from_iter_values(pairs.iter().map(|&(_, d)| d));
    vec![RecordBatch::try_new(schema, vec![Arc::new(src), Arc::new(dst)]).expect("two u32 columns")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(name: &str, dt: Primitive) -> NodeProp {
        NodeProp {
            name: name.to_string(),
            datatype: dt,
            iri: None,
            cardinality: Cardinality::Single,
        }
    }

    #[test]
    fn builds_dense_ids_and_resolves_edges() {
        let cats = LiteralVertex {
            label: "Catalog".into(),
            rdf_type: Some("http://www.w3.org/ns/dcat#Catalog".into()),
            properties: vec![prop("title", Primitive::String)],
            rows: vec![vec!["urn:cat:1".into(), "My Catalog".into()]],
        };
        let datasets = LiteralVertex {
            label: "Dataset".into(),
            rdf_type: Some("http://www.w3.org/ns/dcat#Dataset".into()),
            properties: vec![prop("count", Primitive::Integer)],
            // Out of subject order on purpose → dense id must follow sorted IRI.
            rows: vec![
                vec!["urn:ds:b".into(), "2".into()],
                vec!["urn:ds:a".into(), "9".into()],
            ],
        };
        let edge = LiteralEdge {
            label: "dataset".into(),
            rdf_uri: None,
            src_type: "Catalog".into(),
            dst_type: "Dataset".into(),
            single_valued: false,
            pairs: vec![
                ("urn:cat:1".into(), "urn:ds:a".into()),
                ("urn:cat:1".into(), "urn:ds:b".into()),
                ("urn:cat:1".into(), "urn:missing".into()), // dangling → dropped
            ],
        };

        let graph = from_literal_graph(vec![cats, datasets], vec![edge]);

        // Dataset's `count` is a real Int64 column (not stringly typed).
        let ds = graph
            .vertices
            .iter()
            .find(|v| v.label == "Dataset")
            .unwrap();
        let batch = &ds.batches[0];
        let schema = batch.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            ["dense_id", "subject", "count", "x", "y", "cluster_id"]
        );
        let counts = batch
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        // dense 0 = urn:ds:a (sorted first) carries 9; dense 1 = urn:ds:b carries 2.
        assert_eq!(counts.value(0), 9);
        assert_eq!(counts.value(1), 2);

        // The edge resolved 2 of 3 pairs (the dangling one dropped).
        let e = &graph.edges[0];
        let rows: usize = e.by_source.iter().map(|b| b.num_rows()).sum();
        assert_eq!(rows, 2, "dangling endpoint drops, like the program path");
        assert_eq!(graph.schema.edge("dataset").unwrap().destination, "Dataset");
    }
}
