//! The type `fossil.json` declares for a column is the type the file holds.
//!
//! It was the CHECKER's type (`VProp.ty`), stamped beside a Parquet column the
//! ENGINE encoded, and the two disagreed in two ways: `Integer <: Float` let an
//! `Int64` argument satisfy a `Float` parameter with no widening cast, so a
//! call returned `sig.ret` to the checker and `Int64` to the file
//! (`tests/aggregate_result_types.rs` is that table); and the executor
//! introspects nothing, so an untyped column was declared `string` over a
//! `BIGINT`. `properties` describes the file a reader opens, so the writer
//! spells the written column's Arrow type (`fossil_sinks::manifest::data_type_name`)
//! and both disagreements stop reaching the artefact.
//!
//! The checker's disagreement with the engine is still real, and still in
//! `aggregate_result_types.rs`; it is a question about the language, not about
//! what a corpus says of its own bytes.

// The fixtures use interpolation syntax (`"…{Order.customer}"`), which clippy
// mistakes for format args in a plain string literal.
#![allow(clippy::literal_string_with_formatting_args)]

#[path = "support/native.rs"]
mod native;
mod support;

const SHAPE: &str = "\
PREFIX shop: <https://shop.example/voc#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>

shop:Spend {
  shop:customer xsd:string ;
  shop:total    xsd:float
}
";

/// `amount` is written without a decimal point, so the engine reads the column
/// as `Int64`.
const ORDERS: &str = "\
order_id,customer,amount
10,alice,100
11,bob,250
12,alice,75
13,carol,300
14,alice,25
";

/// Run the fixture and give back `(declared, on_disk)` for `total`: the
/// manifest's `data_type` string, and what `DuckDB` says the Parquet column is —
/// read with plain SQL and never through the writer's own types.
fn declared_and_on_disk(program: &str) -> (String, String) {
    let dir = native::write_dir(&[
        ("shape.shex", SHAPE),
        ("data/orders.csv", ORDERS),
        ("spend.fossil", program),
    ]);
    let corpus = native::run_dir(dir.path(), "spend.fossil", &[]).expect("the program runs");
    let m = corpus.manifest();
    let spend = m
        .vertex_tables
        .iter()
        .find(|v| v.name == "Spend")
        .expect("Spend");
    let declared = spend
        .properties
        .iter()
        .find(|p| p.name == "total")
        .expect("total is declared")
        .data_type
        .clone();

    let root = corpus.materialise();
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let on_disk = conn
        .query_row(
            &format!(
                "SELECT column_type FROM (DESCRIBE SELECT * FROM \
                 read_parquet('{}')) WHERE column_name = 'total'",
                root.path().join(&spend.path).display(),
            ),
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("the column is in the file");
    (declared, on_disk)
}

/// `group_by(Order.customer, total = math.sum(Order.amount))` over an `Int64`
/// column: the checker believes `double`, the engine writes `Int64`, and the
/// manifest says what was written.
#[test]
fn a_group_bys_aggregate_declares_what_the_file_holds() {
    let (declared, on_disk) = declared_and_on_disk(
        "\
type { Spend } := io.shex(\"shape.shex\")

Order := io.csv(\"data/orders.csv\")

PerCustomer := Order.group_by(Order.customer, total = math.sum(Order.amount))

Spending : Spend from PerCustomer
    @subject = \"https://shop.example/spend/{Order.customer}\"
    customer = Order.customer
    total    = PerCustomer.total
",
    );
    assert_eq!(declared, "int64", "the manifest declares the column's type");
    assert_eq!(on_disk, "BIGINT");
}

/// The same with no `group_by` anywhere: `math.abs` is type-preserving too, so
/// the checker's `double` is not what the file holds either.
#[test]
fn a_scalar_call_declares_what_the_file_holds() {
    let (declared, on_disk) = declared_and_on_disk(
        "\
type { Spend } := io.shex(\"shape.shex\")

Order := io.csv(\"data/orders.csv\")

Spending : Spend from Order
    @subject = \"https://shop.example/spend/{Order.order_id}\"
    customer = Order.customer
    total    = math.abs(Order.amount)
",
    );
    assert_eq!(declared, "int64", "the manifest declares the column's type");
    assert_eq!(on_disk, "BIGINT");
}

/// With no call at all, and the source untyped to the executor's checker: the
/// column's type is still the column's (`shop:total` is `xsd:float` in the
/// shape, which is a claim about the graph, not about the file).
#[test]
fn a_bare_column_declares_what_it_holds() {
    let (declared, on_disk) = declared_and_on_disk(
        "\
type { Spend } := io.shex(\"shape.shex\")

Order := io.csv(\"data/orders.csv\")

Spending : Spend from Order
    @subject = \"https://shop.example/spend/{Order.order_id}\"
    customer = Order.customer
    total    = Order.amount
",
    );
    assert_eq!(declared, "int64", "the manifest declares the column's type");
    assert_eq!(on_disk, "BIGINT");
}
