//! The type a property is written as is its primitive's, and the file holds it.
//!
//! Not the type the ENGINE encoded: `Integer <: Float` lets an `Int64` satisfy a
//! `Float`, and a source hands the run whatever its reader inferred, so the
//! values of one property can arrive as `Int64` from one file and `Float64`
//! from another. A graph's schema is its program's and does not change with
//! the files it was built from, so the writer stores each primitive as one
//! `GraphAr` type (`fossil_sinks::graphar::layout`, `stored_type`). Every
//! assertion here reads the file with plain SQL on `DuckDB`, takes the Arrow
//! type it answers each column in, and never looks at the writer's own types.

// The fixtures use interpolation syntax (`"…{Order.customer}"`), which clippy
// mistakes for format args in a plain string literal.
#![allow(clippy::literal_string_with_formatting_args)]

use std::collections::HashMap;

use datafusion::arrow::datatypes::DataType;
use fossil_graph_schema::{OutsideLattice, Primitive};
use fossil_profile::primitive;

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
/// information file's `data_type`, and the Arrow type `DuckDB` answers the
/// Parquet column in — read with plain SQL and never through the writer's own
/// types.
fn declared_and_on_disk(program: &str) -> (String, DataType) {
    let dir = native::write_dir(&[
        ("shape.shex", SHAPE),
        ("data/orders.csv", ORDERS),
        ("spend.fossil", program),
    ]);
    let ran = native::run_dir(dir.path(), "spend.fossil", &[]).expect("the program runs");
    let mut columns = written(&ran, "Spend");
    columns.remove("total").expect("total is written")
}

/// Every property of vertex type `label`: the information file's `data_type`,
/// and the Arrow type `DuckDB` answers the Parquet column in.
fn written(ran: &native::Written, label: &str) -> HashMap<String, (String, DataType)> {
    let root = ran.materialise();
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let path = root.path().join(ran.chunk(label));
    let mut stmt = conn
        .prepare(&format!(
            "SELECT * FROM read_parquet('{}') LIMIT 0",
            path.display()
        ))
        .expect("the chunk is read");
    let schema = stmt
        .query_arrow([])
        .expect("the chunk is read")
        .get_schema();
    ran.vertex(label).property_groups[0]
        .properties
        .iter()
        .map(|p| {
            let field = schema
                .field_with_name(&p.name)
                .expect("the column is in the file");
            // A set is a list of its element, which is the property's
            // cardinality and not its type.
            let on_disk = match (p.cardinality, field.data_type()) {
                (graphar_info::Cardinality::Single, t) => t,
                (_, duckdb::arrow::datatypes::DataType::List(element)) => element.data_type(),
                (_, t) => panic!("`{}` holds a set, and `{t}` is not a list", p.name),
            };
            // `duckdb` answers in its own arrow (58), the profiler reads
            // DataFusion's (60): the type crosses as the text both print and
            // parse, which names every parameter.
            let on_disk = on_disk.to_string().parse().expect("an Arrow type");
            (p.name.clone(), (p.data_type.to_string(), on_disk))
        })
        .collect()
}

/// `group_by(Order.customer, total = math.sum(Order.amount))` over an `Int64`
/// column: the engine computes an `Int64`, and `total` is a `Float`.
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
    assert_eq!(
        declared, "double",
        "the information file declares the property's type"
    );
    assert_eq!(primitive(&on_disk), Ok(Primitive::Float), "{on_disk}");
}

/// The same with no `group_by` anywhere: `math.abs` is type-preserving, and
/// the file is still the property's.
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
    assert_eq!(
        declared, "double",
        "the information file declares the property's type"
    );
    assert_eq!(primitive(&on_disk), Ok(Primitive::Float), "{on_disk}");
}

/// With no call at all: the source's `Int64` column is written as the
/// property's `Float`.
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
    assert_eq!(
        declared, "double",
        "the information file declares the property's type"
    );
    assert_eq!(primitive(&on_disk), Ok(Primitive::Float), "{on_disk}");
}

const EVERY: &str = "\
PREFIX ex:  <https://example.org/voc#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>

ex:Thing {
  ex:n  xsd:integer ;
  ex:x  xsd:double ;
  ex:ok xsd:boolean ;
  ex:y  xsd:gYear ;
  ex:d  xsd:date ;
  ex:t  xsd:dateTime ;
  ex:s  xsd:string ;
  ex:l  rdf:langString
}
";

/// The text columns carry a `v` so no reader takes them for a date: the program
/// says what each one is.
const THINGS: &str = "\
id,n,x,ok,y,d,t,s
1,7,1.5,true,v1999,v1999-01-02,v1999-01-02 03:04:05,hi
";

/// **A graph fossil wrote is profiled as its program declared it.** Each
/// primitive's column, written by a run and read by `DuckDB`, is answered in an
/// Arrow type the profiler reads back as that primitive — but for the three the
/// lattice keeps apart only by meaning: a year is stored as an integer, and a
/// tagged text as `GraphAr`'s `langstring`, a struct no primitive holds.
#[test]
fn every_primitive_is_profiled_as_it_was_declared() {
    let dir = native::write_dir(&[
        ("shape.shex", EVERY),
        ("data/things.csv", THINGS),
        (
            "things.fossil",
            "\
type { Thing } := io.shex(\"shape.shex\")

Row := io.csv(\"data/things.csv\")

Things : Thing from Row
    @subject = \"https://example.org/thing/{Row.id}\"
    n  = Row.n
    x  = Row.x
    ok = Row.ok
    y  = parse.year(str.slice(Row.y, 2))
    d  = parse.date(str.slice(Row.d, 2), \"%Y-%m-%d\")
    t  = parse.datetime(str.slice(Row.t, 2), \"%Y-%m-%d %H:%M:%S\")
    s  = Row.s
    l  = str.lang(Row.s, \"en\")
",
        ),
    ]);
    let ran = native::run_dir(dir.path(), "things.fossil", &[]).expect("the program runs");
    let written = written(&ran, "Thing");
    for (name, declared, read) in [
        ("n", Primitive::Integer, Ok(Primitive::Integer)),
        ("x", Primitive::Float, Ok(Primitive::Float)),
        ("ok", Primitive::Bool, Ok(Primitive::Bool)),
        ("y", Primitive::YEAR, Ok(Primitive::Integer)),
        ("d", Primitive::DATE, Ok(Primitive::DATE)),
        ("t", Primitive::DATE_TIME, Ok(Primitive::DATE_TIME)),
        ("s", Primitive::STRING, Ok(Primitive::STRING)),
        ("l", Primitive::LANG_STRING, Err(OutsideLattice)),
    ] {
        let (_, on_disk) = &written[name];
        assert_eq!(
            primitive(on_disk),
            read,
            "`{name}`, a {declared}, is `{on_disk}`"
        );
    }
}
