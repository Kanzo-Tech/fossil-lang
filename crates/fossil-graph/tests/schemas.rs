//! Snapshot the JSON Schemas for every verb's params + result.
//!
//! Per ADR-0039: these snapshots ARE the wire contract every transport
//! binding consumes. A hand-edit to a `Params`/`Result` struct that the
//! author didn't intend to publish surfaces as a snapshot diff and fails CI
//! until reviewed (`cargo insta review` to accept).
//!
//! Downstream consumers (fossil-mcp, fossil-http, @fossil-lang/graph
//! codegen) treat the accepted `.snap` files as the source of truth. The
//! TS bindings, in particular, regenerate their wrapper types from these
//! schemas via openapi-typescript at the playground/keasy `pnpm openapi`
//! step.

use fossil_graph::operations::{Operation, aggregate, discovery, graphrag, schema, sql, viewport};
use schemars::schema_for;

macro_rules! snap {
    ($name:literal, $ty:ty) => {
        insta::assert_yaml_snapshot!($name, schema_for!($ty));
    };
}

#[test]
fn dispatch_envelope() {
    // The outer Operation enum — its tagged shape IS the wire envelope
    // (`{ "verb": "...", "params": {...} }`).
    snap!("operation_envelope", Operation);
}

#[test]
fn schema_verbs() {
    snap!("list_vertex_types_params", schema::ListVertexTypesParams);
    snap!("list_vertex_types_result", schema::ListVertexTypesResult);
    snap!("list_edge_types_params", schema::ListEdgeTypesParams);
    snap!("list_edge_types_result", schema::ListEdgeTypesResult);
    snap!("describe_field_params", schema::DescribeFieldParams);
    snap!("describe_field_result", schema::DescribeFieldResult);
    snap!(
        "describe_vertex_type_params",
        schema::DescribeVertexTypeParams
    );
    snap!(
        "describe_vertex_type_result",
        schema::DescribeVertexTypeResult
    );
}

#[test]
fn discovery_verbs() {
    snap!("search_by_label_params", discovery::SearchByLabelParams);
    snap!("search_by_label_result", discovery::SearchByLabelResult);
    snap!("find_neighbors_params", discovery::FindNeighborsParams);
    snap!("find_neighbors_result", discovery::FindNeighborsResult);
    snap!("find_path_params", discovery::FindPathParams);
    snap!("find_path_result", discovery::FindPathResult);
    snap!("get_vertex_params", discovery::GetVertexParams);
    snap!("get_vertex_result", discovery::GetVertexResult);
}

#[test]
fn aggregate_verbs() {
    snap!("aggregate_params", aggregate::AggregateParams);
    snap!("aggregate_result", aggregate::AggregateResult);
    snap!("histogram_params", aggregate::HistogramParams);
    snap!("histogram_result", aggregate::HistogramResult);
    snap!("top_k_params", aggregate::TopKParams);
    snap!("top_k_result", aggregate::TopKResult);
}

#[test]
fn graphrag_verbs() {
    snap!("summarize_cluster_params", graphrag::SummarizeClusterParams);
    snap!("summarize_cluster_result", graphrag::SummarizeClusterResult);
    snap!(
        "answer_with_communities_params",
        graphrag::AnswerWithCommunitiesParams
    );
    snap!(
        "answer_with_communities_result",
        graphrag::AnswerWithCommunitiesResult
    );
}

#[test]
fn viewport_verbs() {
    snap!("viewport_params", viewport::ViewportParams);
    snap!("viewport_result", viewport::ViewportResult);
    snap!("set_selection_params", viewport::SetSelectionParams);
    snap!("set_selection_result", viewport::SetSelectionResult);
    snap!("materialize_graph_params", viewport::MaterializeGraphParams);
    snap!("materialize_graph_result", viewport::MaterializeGraphResult);
}

#[test]
fn sql_verb() {
    snap!("execute_sql_params", sql::ExecuteSqlParams);
    snap!("execute_sql_result", sql::ExecuteSqlResult);
}

#[test]
fn operation_round_trip() {
    // Sanity: the wire envelope round-trips through JSON.
    let op = Operation::ListVertexTypes(schema::ListVertexTypesParams {});
    let json = serde_json::to_string(&op).expect("serialize");
    assert!(
        json.contains(r#""verb":"list_vertex_types""#),
        "envelope tag should match snake_case verb name, got: {json}",
    );
    let back: Operation = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.verb_name(), "list_vertex_types");
}
