//! **An edge whose endpoint is not a vertex is dropped, and the run says how
//! many.**
//!
//! `execute_edge` resolves both endpoints with an inner join, so a row naming a
//! subject no vertex carries never becomes an edge. That is the ruling and it
//! stands — a corpus cannot hold an edge to a vertex that is not there. What
//! changed is that it stopped being silent: the join reported nothing, at any
//! log level, and the only trace left was an `edge_count` smaller than the
//! source's row count, which nobody has to compare against anything.
//!
//! Measured on `apps/docs/programs/reviews/` before the count existed: three
//! reviews, two people, `edge_count: 2`, and no output of any kind naming the
//! third.
//!
//! # What this cannot prove
//!
//! The number is `candidate rows − resolved rows`, so it is exact only while a
//! subject identifies at most one vertex. A vertex type materialised without
//! dedup can hold two rows with the same `subject`, and then one candidate
//! resolves to two edges and the subtraction under-reports — saturating to zero
//! while rows were both duplicated and dropped. That corpus already violates
//! `identity-is-the-subject` (`apps/corpus/guards/guards.mjs`), which is the
//! guard that catches it; nothing here does.
//!
//! It also says nothing about WHICH endpoint dangled, or which subject: a count
//! is what the writer can produce without holding the unresolved rows, and
//! holding them is a second copy of the input.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::literal_string_with_formatting_args)]

use datafusion::prelude::SessionContext;

mod support;

const GRAPH_SHEX: &str = include_str!("fixtures/graph.shex");

/// `orders_dangling.csv` names user 99, and `users.csv` stops at 3.
const PROGRAM: &str = "\
type { Person, Order } := io.shex(\"graph.shex\")

users := io.csv(\"tests/fixtures/users.csv\")
orders := io.csv(\"tests/fixtures/orders_dangling.csv\")

Person : Person from users
    @subject = \"https://example.org/person/{users.id}\"
    name = users.name

Order : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = \"https://example.org/person/{orders.user_id}\"
    total = orders.amount
";

/// The same program over `orders.csv`, whose every `user_id` is a real person.
const CLEAN_PROGRAM: &str = "\
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

async fn run(program: &str) -> fossil_df::GraphArData {
    let (db, file) =
        support::db_with_shapes(program, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);
    let ctx = SessionContext::new();
    fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &fossil_df::OutputDescriptorKind::ShEx(
            fossil_shex::ShExDescriptor::from_shex_source(GRAPH_SHEX).expect("parse graph.shex"),
        ),
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)))
}

#[tokio::test]
async fn a_dangling_endpoint_is_counted_rather_than_swallowed() {
    let graph = run(PROGRAM).await;

    let edge = &graph.edges[0];
    assert_eq!(edge.label, "placedBy");
    // Four order rows in, three edges out — the ruling: the inner join stays.
    assert_eq!(
        edge.by_source.iter().map(|b| b.num_rows()).sum::<usize>(),
        3,
        "the row naming person/99 is not an edge, because person/99 is not a vertex"
    );
    assert_eq!(
        edge.dropped, 1,
        "and the one that was discarded is on the record"
    );

    // The report is what a caller reads, and it names the manifest entry the
    // count belongs to rather than re-spelling the edge's triple.
    let report = fossil_df::RunReport::of("file:///tmp/x", &graph);
    assert_eq!(
        report.dropped,
        vec![fossil_df::report::EdgeDrops {
            prefix: "edge/Order_placedBy_Person/".to_string(),
            dropped: 1,
        }],
    );
    assert_eq!(
        report.edges[0].prefix, report.dropped[0].prefix,
        "the drop is keyed by the prefix the manifest declares"
    );
}

/// And zero is a number the report states, not one it omits — a reader that has
/// to tell «nothing was dropped» from «this writer does not count» has the same
/// silence back in a different shape.
#[tokio::test]
async fn a_clean_run_reports_zero_rather_than_nothing() {
    let graph = run(CLEAN_PROGRAM).await;
    assert_eq!(graph.edges[0].dropped, 0);
    let report = fossil_df::RunReport::of("file:///tmp/x", &graph);
    assert_eq!(
        report.dropped,
        vec![fossil_df::report::EdgeDrops {
            prefix: "edge/Order_placedBy_Person/".to_string(),
            dropped: 0,
        }],
    );
}
