//! Does the type the checker believes reach the ARTEFACT, or does it stop at
//! the ergonomics? — the question that decides how urgent the `math.sum`
//! disagreement is.
//!
//! It reaches the artefact. A property's `data_type` in `*.vertex.yml` is
//! stamped from `VProp.ty`, which is the CHECKER's type
//! (`crates/fossil-df/src/lib.rs`, `node_property` → `inner_primitive`), while
//! the Parquet column beside it is encoded from the Arrow type the ENGINE
//! produced (`crates/fossil-df/src/files.rs`). Nothing in the tree compares the
//! two — `fossil_sinks::manifest::data_type_name`'s own doc comment says so,
//! `tests/conformance.rs` checks paths, counts, tiling and the adjacency joins,
//! and `apps/corpus/guards/guards.mjs` records the same gap in its
//! `cannotProve`.
//!
//! # The mechanism, and it is not about aggregates
//!
//! `Integer <: Float` is a real rule of the language
//! (`fossil_hir::check::subtypes`, S-IntFlt), so an `Int64` column satisfies a
//! `Float` parameter. Nothing then WIDENS it: neither the catalogue's template
//! nor `fossil-df`'s lowering inserts a cast at the argument, and every `math/`
//! row but `avg` and `round` is type-preserving in both engines. So the row
//! returns `sig.ret` to the checker and the argument's own type to the file.
//!
//! `crates/fossil-df/tests/aggregate_result_types.rs` is the type table this
//! stands on — sixteen cells, ten disagreeing. This file is the other half: the
//! same disagreement, followed all the way to a byte on disk.
//!
//! # Why both a scalar and an aggregate
//!
//! `group_by` is where it was NOTICED, and it is not where it lives.
//! [`a_scalar_call_writes_the_same_lie`] has no `group_by` in it at all. If the
//! two ever diverge — one fixed and the other not — the pair says which.
//!
//! # These tests assert TODAY's answer
//!
//! Each is written so that closing the disagreement turns it red, whichever way
//! it is closed. That is deliberate: a defect nobody can see is what put it
//! here, and a silent repair would leave the table above quietly wrong.

#![cfg(not(target_arch = "wasm32"))]
// The fixtures use interpolation syntax (`"…{Order.customer}"`), which clippy
// mistakes for format args in a plain string literal.
#![allow(clippy::literal_string_with_formatting_args)]

use std::path::Path;

use duckdb::Connection;

const SHAPE: &str = "\
PREFIX shop: <https://shop.example/voc#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>

shop:Spend {
  shop:customer xsd:string ;
  shop:total    xsd:float
}
";

/// `amount` is written without a decimal point, so `DESCRIBE` types the column
/// `BIGINT` and the checker sees an `Integer` — which is the whole setup.
const ORDERS: &str = "\
order_id,customer,amount
10,alice,100
11,bob,250
12,alice,75
13,carol,300
14,alice,25
";

/// Write the fixture, run it, and give back `(declared, on_disk)` for `total`.
///
/// `declared` is the manifest's `data_type` string; `on_disk` is what `DuckDB`
/// says the Parquet column is — read with plain SQL and never through our own
/// reader, for the reason `tests/conformance.rs` gives at length.
fn declared_and_on_disk(program: &str) -> (String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("data")).expect("data dir");
    std::fs::write(root.join("shape.shex"), SHAPE).expect("shape");
    std::fs::write(root.join("data/orders.csv"), ORDERS).expect("orders");
    let path = root.join("spend.fossil");
    std::fs::write(&path, program).expect("program");

    // Introspect first, as `fossil-cli` does: without it the sources carry no
    // forward-propagated types and `amount` would not be an `Integer` at all.
    let system = fossil_engine::host_system(&path);
    let _ = fossil_introspect::introspect_program(
        &*system,
        &path,
        &std::collections::HashMap::new(),
        &fossil_introspect::RunCreds::default(),
    );

    let dest = root.join("out");
    fossil_engine::run(
        &path,
        &format!("file://{}", dest.display()),
        &std::collections::HashMap::new(),
        None,
        None,
    )
    .expect("fossil run");

    (
        manifest_data_type(&dest.join("vertex/Spend.vertex.yml"), "total"),
        parquet_column_type(&dest.join("vertex/Spend/tiles.parquet"), "total"),
    )
}

/// The `data_type:` the manifest declares for one property.
///
/// Parsed off the YAML text rather than through `fossil_sinks`'s structs: what
/// is being checked is the promise a STRANGER reads, and a stranger has the
/// file.
fn manifest_data_type(manifest: &Path, property: &str) -> String {
    let yaml = std::fs::read_to_string(manifest).expect("the vertex manifest");
    let mut lines = yaml
        .lines()
        .skip_while(|l| l.trim() != format!("- name: {property}"));
    lines.next().expect("the property's own line");
    lines
        .next()
        .expect("the line after it")
        .trim()
        .strip_prefix("data_type: ")
        .expect("`data_type` is the line after `name` in a property group")
        .to_string()
}

/// What `DuckDB` reads the column back as.
fn parquet_column_type(payload: &Path, column: &str) -> String {
    let conn = Connection::open_in_memory().expect("duckdb");
    conn.query_row(
        &format!(
            "SELECT column_type FROM (DESCRIBE SELECT * FROM \
             read_parquet('{}')) WHERE column_name = '{column}'",
            payload.display(),
        ),
        [],
        |row| row.get::<_, String>(0),
    )
    .expect("the column is in the file")
}

/// `group_by(Order.customer, total = math.sum(Order.amount))` over an `Integer`
/// column: the manifest promises `double` and the file holds `int64`.
///
/// Measured 2026-08-26 on the branch that landed `group_by`. The values are
/// right (`alice` is 200); it is only the type that is a lie.
#[test]
fn a_group_bys_aggregate_writes_a_type_the_file_does_not_hold() {
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
        "the manifest stamps `math.sum`'s `sig.ret`"
    );
    assert_eq!(
        on_disk, "BIGINT",
        "DataFusion's `sum` over an `Int64` column is an `Int64` — if this is \
         now `DOUBLE`, the disagreement was closed by coercing in the lowering",
    );
}

/// The same lie with no `group_by` anywhere: `math.abs` is type-preserving too.
///
/// This is why the fix does not belong to the aggregates. Four rows of `math/`
/// can write it — `sum`, `min`, `max`, `abs` — and `avg` cannot, because it
/// returns a `Float64` from an `Int64` in both engines.
#[test]
fn a_scalar_call_writes_the_same_lie() {
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
    assert_eq!(declared, "double");
    assert_eq!(on_disk, "BIGINT");
}

/// And with no CALL at all the two AGREE — which is what says the defect is the
/// call's declared return and not the shape document.
///
/// `shop:total` is `xsd:float` in the shape either way. The property type comes
/// from the expression, so a bare column writes `int64` over an `int64` and
/// nothing is wrong. A reader of this file should not conclude that the shape
/// is being ignored: it is the checker's inferred type that is stamped, and for
/// a bare column that type is the column's.
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
    assert_eq!(declared, "int64");
    assert_eq!(on_disk, "BIGINT");
}
