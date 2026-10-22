// `{people.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! **A two-mapping program whose edge is a CALL**, through the executor.
//!
//! `placedBy = Person(orders.user_id)` is the destination type applied to an
//! expression, resolved through the one identity template that type has. The
//! `.shex` is the other half — `ex:placedBy @ex:Person` is what classifies the
//! predicate as an edge rather than a property, and `ex:amount` beside it stays
//! a column. Nothing guesses it by matching a string template against every
//! mapping's subject template.
//!
//! And `mapping.ttl` carries what the governance layer reads off a graph: the
//! full type IRI per vertex type, and each property's predicate.

#[path = "support/native.rs"]
mod native;
mod support;

const PROGRAM: &str = "\
type { Person, Order } := io.shex(\"prog.shex\")

people := io.csv(\"people.csv\")
orders := io.csv(\"orders.csv\")

People : Person from people
    @subject = \"https://example.org/person/{people.id}\"
    name = people.name

Orders : Order from orders
    @subject = \"https://example.org/order/{orders.order_id}\"
    placedBy = Person(orders.user_id)
    amount = orders.amount
";

const SHEX: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string
}

ex:Order {
  ex:placedBy @ex:Person ;
  ex:amount   .
}
";

#[test]
fn a_type_call_writes_a_cross_type_edge_from_two_mappings() {
    let dir = native::write_dir(&[
        ("prog.fossil", PROGRAM),
        ("prog.shex", SHEX),
        ("people.csv", "id,name\n1,Ada\n2,Linus\n3,Grace\n"),
        (
            "orders.csv",
            "order_id,user_id,amount\no1,1,10\no2,2,20\no3,1,30\n",
        ),
    ]);
    let ran = native::run_dir(dir.path(), "prog.fossil", &[]).expect("the program runs");

    let order = ran.vertex("Order");
    let person = ran.vertex("Person");
    assert_eq!(
        (ran.vertex_count("Person"), ran.vertex_count("Order")),
        (3, 3)
    );

    // Three orders, each naming a real person: three edges, none dropped —
    // stated rather than omitted, because «no key» and «nothing dropped» are
    // not the same answer.
    let graph = ran.graph();
    let placed_by = graph
        .edges
        .iter()
        .find(|e| e.src_type == "Order" && e.dst_type == "Person")
        .unwrap_or_else(|| panic!("no Order→Person edge: {graph:?}"));
    assert_eq!(placed_by.edge_type, "placedBy");
    assert_eq!(ran.edge_count(&placed_by.triple()), 3);
    let drops = ran
        .report
        .dropped
        .iter()
        .find(|d| d.table == placed_by.triple())
        .expect("a drop count for the edge");
    assert_eq!(drops.dropped, 0);

    // The literal `ex:amount` stayed a property on Order; `ex:placedBy` did not.
    let order_columns: Vec<&str> = order.property_groups[0]
        .properties
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(order_columns.contains(&"amount"), "{order_columns:?}");
    assert!(!order_columns.contains(&"placedBy"), "{order_columns:?}");

    // A property's storage type. (The type's IRI is the mapping's, not the YAML's.)
    let name = person.property_groups[0]
        .properties
        .iter()
        .find(|p| p.name == "name")
        .expect("name on Person");
    assert_eq!(name.data_type.to_string(), "string");
}
