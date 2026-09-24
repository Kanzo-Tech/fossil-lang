//! Native sink: `execute_graph` → `write_manifests` lays the dataset's manifests
//! on disk — a YAML per vertex type, one per edge directory, and the graph index
//! — describing the rows the executor materialised.
//!
//! **It writes no Parquet, and this asserts that too.** The sink emitted one per
//! vertex type and a pair per edge, `fossil_layout`'s pass read every one of
//! them back, rewrote it as tiles and unlinked it. The payload is now written
//! once, by the pass, out of the same batches; what tests THAT is
//! `fossil-layout`'s own suites and `fossil-cli`'s `walking_skeleton` /
//! `conformance`, which read a finished corpus.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use std::fs;

use datafusion::prelude::SessionContext;

mod support;

const PROGRAM: &str = "\
type { Person, Order } := io.shex(\"graph.shex\")

users := io.csv(\"tests/fixtures/users.csv\")
orders := io.csv(\"tests/fixtures/orders.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Order : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = \"https://example.org/person/{orders.user_id}\"
    total = orders.amount
";

/// The document the program names — shared with `execute_graph.rs`, and the
/// same text goes to the executor as the descriptor. `ex:placedBy @ex:Person`
/// is what makes `placedBy` an edge; under `ACCEPT_ALL_DEFAULT` it degrades to
/// a string column and the whole `edge/` half of the tree asserted below
/// silently stops existing.
const GRAPH_SHEX: &str = include_str!("fixtures/graph.shex");

#[tokio::test]
async fn write_manifests_lays_out_the_graphar_manifests_and_no_payload() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);
    let descriptor = fossil_df::OutputDescriptorKind::Lowered(
        fossil_shex::ShExDescriptor::from_shex_source(GRAPH_SHEX)
            .expect("parse graph.shex")
            .to_graph_schema(&fossil_graph_schema::Renames::default()),
    );

    let ctx = SessionContext::new();
    let graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &descriptor,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    let dir = tempfile::tempdir().expect("tempdir");
    graph.write_manifests(dir.path()).expect("write_manifests");

    // Every manifest of the tree exists.
    for rel in [
        "graph.graph.yml",
        "vertex/Person.vertex.yml",
        "vertex/Order.vertex.yml",
        "edge/Order_placedBy_Person/Order_placedBy_Person.edge.yml",
    ] {
        assert!(dir.path().join(rel).exists(), "missing {rel}");
    }

    // **And no payload, which is the phase order made assertable.** A staged
    // vertex Parquet beside the tiles that replace it is two containers for one
    // set of rows, which is what `apps/corpus`'s `exactly-once` and
    // `declared-tiling` fail a corpus for. It used to be deleted by whoever ran
    // the layout afterwards; it is not written.
    for rel in [
        "vertex/Person.parquet",
        "vertex/Order.parquet",
        "edge/Order_placedBy_Person/by_source.parquet",
        "edge/Order_placedBy_Person/by_target.parquet",
    ] {
        assert!(
            !dir.path().join(rel).exists(),
            "{rel} was staged; the payload is the layout pass's to write"
        );
    }

    // The rows the manifests describe are in hand, which is how the pass gets
    // them. Counted off the batches because that is where they are — this used
    // to read the staged Parquet back, and the staged Parquet was these batches
    // encoded.
    let rows: usize = graph
        .vertices
        .iter()
        .find(|v| v.label == "Person")
        .expect("a Person table")
        .batches
        .iter()
        .map(datafusion::arrow::record_batch::RecordBatch::num_rows)
        .sum();
    assert_eq!(rows, 3, "users.csv → 3 Person vertices");

    let yaml = fs::read_to_string(dir.path().join("vertex/Person.vertex.yml")).unwrap();
    assert!(yaml.contains("type: Person"), "{yaml}");
    assert!(yaml.contains("name: dense_id"), "{yaml}");

    // **`is_primary` marks exactly one property and it is `subject`.**
    //
    // Read back through the structs rather than by substring, because the
    // question is which property carries the flag and a `contains` cannot say:
    // `is_primary: true` is in this file either way. Asserted here because this
    // is the test that holds the bytes a run wrote, and the flag is a claim
    // about the artefact.
    //
    // Nothing in the tree READS it — `packages/corpus`'s reader keys on the
    // column name, and it says why: it could not trust a field two writers
    // spelled two ways. So this is the only thing that goes red if `dense_id`
    // takes the flag back, and it is deliberately the artefact's own manifest
    // and not `vertex_info`'s return value.
    let info: fossil_sinks::manifest::VertexInfo =
        serde_yaml_ng::from_str(&yaml).unwrap_or_else(|e| panic!("Person.vertex.yml: {e}\n{yaml}"));
    let primary: Vec<&str> = info
        .properties()
        .iter()
        .filter(|p| p.is_primary)
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        primary,
        ["subject"],
        "the identity is the subject IRI; dense_id is an address a re-layout gives away\n{yaml}"
    );

    // Edges: 4 orders → 4 CSR rows, in both orientations.
    let edge = graph
        .edges
        .iter()
        .find(|e| e.label == "placedBy")
        .expect("a placedBy relation");
    for (orientation, batches) in [
        ("by_source", &edge.by_source),
        ("by_target", &edge.by_target),
    ] {
        let rows: usize = batches
            .iter()
            .map(datafusion::arrow::record_batch::RecordBatch::num_rows)
            .sum();
        assert_eq!(rows, 4, "{orientation}");
    }
}

/// **A channel the pass measured survives the trip to disk, and a type it never
/// reached declares nothing.**
///
/// `sink.rs` is the test that holds the bytes a run wrote, and a `domain` is a
/// claim about the artefact in exactly the way `is_primary` above is: it is read
/// back through the structs rather than by substring, because what is asserted
/// is which channel carries which number and a `contains` cannot say.
///
/// The second half is the one the field's three states exist for. `Order` is a
/// type the layout pass did not report on, and the manifest it gets must have **no
/// `channels:` key** — not an empty list. The two are different sentences: no key
/// is *nobody said*, and an empty list is *this type carries none*. A writer that
/// wrote `channels: []` for every silent type would make a corpus predating the
/// field indistinguishable from one whose author looked and found nothing, which
/// is the whole of what keeps this a declaration rather than a flag day
/// (`/docs/design/position`).
#[tokio::test]
async fn a_measured_channel_reaches_the_manifest_and_a_silent_type_declares_none() {
    let (db, file) =
        support::db_with_shapes(PROGRAM, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);
    let descriptor = fossil_df::OutputDescriptorKind::Lowered(
        fossil_shex::ShExDescriptor::from_shex_source(GRAPH_SHEX)
            .expect("parse graph.shex")
            .to_graph_schema(&fossil_graph_schema::Renames::default()),
    );

    let ctx = SessionContext::new();
    let mut graph = fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &descriptor,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)));

    // What the layout pass hands back for one of the two types, in the shape
    // `LayoutReport::channels` carries — a domain is a count of distinct values
    // and nothing here is allowed to have planned it.
    graph.declare_channels(vec![(
        "Person".to_string(),
        vec![
            fossil_sinks::manifest::Channel::categorical("community", "cluster_id", 3)
                .derived_by("louvain-cut"),
        ],
    )]);

    let dir = tempfile::tempdir().expect("tempdir");
    graph.write_manifests(dir.path()).expect("write_manifests");

    let yaml = fs::read_to_string(dir.path().join("vertex/Person.vertex.yml")).unwrap();
    let info: fossil_sinks::manifest::VertexInfo =
        serde_yaml_ng::from_str(&yaml).unwrap_or_else(|e| panic!("Person.vertex.yml: {e}\n{yaml}"));
    let channels = info
        .channels
        .unwrap_or_else(|| panic!("Person declares its channels\n{yaml}"));
    assert_eq!(channels.len(), 1, "one column, one channel\n{yaml}");
    assert_eq!(channels[0].name, "community");
    assert_eq!(channels[0].column, "cluster_id");
    assert_eq!(
        channels[0].scale,
        fossil_sinks::manifest::Scale::Categorical
    );
    assert_eq!(
        channels[0].domain,
        Some(3),
        "the measured domain is what the document states\n{yaml}"
    );
    assert_eq!(channels[0].derived_by.as_deref(), Some("louvain-cut"));

    let silent = fs::read_to_string(dir.path().join("vertex/Order.vertex.yml")).unwrap();
    let order: fossil_sinks::manifest::VertexInfo = serde_yaml_ng::from_str(&silent)
        .unwrap_or_else(|e| panic!("Order.vertex.yml: {e}\n{silent}"));
    assert_eq!(
        order.channels, None,
        "a type the pass did not reach says nothing, and `channels: []` is saying something\n{silent}"
    );
}
