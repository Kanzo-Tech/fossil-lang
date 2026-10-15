//! **An edge whose endpoint is not a vertex is dropped, and the run says how
//! many.**
//!
//! `execute_edge` resolves both endpoints with an inner join, so a row naming a
//! subject no vertex carries never becomes an edge — a corpus cannot hold an
//! edge to a vertex that is not there. It is not silent: without the count the
//! only trace would be an `edge_count` smaller than the source's row count,
//! which nobody has to compare against anything. `docs/programs/reviews/` is
//! the conformance program that drops one, and its `compiled.txt` carries the
//! count; this file holds what that rendering cannot see — the in-memory
//! `Edge::dropped`, and that the report lists exactly the one table.
//!
//! That a clean run states `0` rather than omitting it is asserted by
//! `execute_graph.rs`, over the same program with `orders.csv`.
//!
//! # What this cannot prove
//!
//! The number is `candidate rows − resolved rows`, so it is exact only while a
//! subject identifies at most one vertex. A vertex type materialised without
//! dedup can hold two rows with the same `subject`, and then one candidate
//! resolves to two edges and the subtraction under-reports — saturating to zero
//! while rows were both duplicated and dropped. That graph already violates
//! admissibility A1 (`/docs/design/graphar`), which the writer refuses as
//! `write/duplicate-value`; nothing here does.
//!
//! It also says nothing about WHICH endpoint dangled, or which subject: a count
//! is what the writer can produce without holding the unresolved rows, and
//! holding them is a second copy of the input.

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

// The executor's futures are not `Send` and are not meant to be — see the
// crate-level allow in `src/lib.rs`. A test that awaits one inherits the lint.
#[allow(clippy::future_not_send)]
async fn run(program: &str) -> fossil_df::Graph {
    let (db, file) =
        support::db_with_shapes(program, "graph.fossil", &[("graph.shex", GRAPH_SHEX)]);
    let ctx = SessionContext::new();
    fossil_df::execute_graph(
        &ctx,
        &db,
        file,
        &fossil_hir::shex::ShExDescriptor::from_shex_source(GRAPH_SHEX)
            .expect("parse graph.shex")
            .to_graph_schema(),
    )
    .await
    .unwrap_or_else(|e| panic!("execute_graph: {e}; {:#?}", support::diagnostics(&db, file)))
}

#[tokio::test]
async fn a_dangling_endpoint_is_counted_rather_than_swallowed() {
    let graph = run(PROGRAM).await;

    let edge = &graph.edges[0];
    assert_eq!(edge.label, "placedBy");
    // Four order rows in, three edges out: the inner join stays.
    assert_eq!(
        edge.batches
            .iter()
            .map(datafusion::arrow::array::RecordBatch::num_rows)
            .sum::<usize>(),
        3,
        "the row naming person/99 is not an edge, because person/99 is not a vertex"
    );
    assert_eq!(
        edge.dropped, 1,
        "and the one that was discarded is on the record"
    );

    // What the write answers is what a caller reads, and it names the edge
    // type the count belongs to rather than re-spelling the edge's triple.
    let (written, files) = support::write_in_memory(&graph).await;
    assert_eq!(
        written.dropped,
        vec![fossil_df::EdgeDrops {
            table: "Order_placedBy_Person".to_string(),
            dropped: 1,
        }],
    );
    assert_eq!(
        written.info.edges[0].triple(),
        written.dropped[0].table,
        "the drop is keyed by the edge type the graph declares"
    );
    assert_eq!(
        support::edge_count(
            &|path| files.get(path).cloned(),
            &written.info,
            &written.dropped[0].table
        ),
        3
    );
}
