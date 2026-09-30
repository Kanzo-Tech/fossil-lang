// `{people.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

//! **A two-mapping program whose edge is a CALL**, through the executor.
//!
//! `placedBy = Person(orders.user_id)` is the destination type applied to an
//! expression, resolved through the one identity template that type has. The
//! `.shex` is the other half — `ex:placedBy @ex:Person` is what classifies the
//! predicate as an edge rather than a property, and `ex:amount` beside it stays
//! a column. It used to be GUESSED, by matching a string template against every
//! mapping's subject template.
//!
//! And `fossil.json` carries what the governance layer reads off a corpus: the
//! full type IRI per vertex table, and each property's storage type.

#![cfg(not(target_arch = "wasm32"))]

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
    let corpus = native::run_dir(dir.path(), "prog.fossil", &[]).expect("the program runs");
    let m = corpus.manifest();

    let order = m
        .vertex_tables
        .iter()
        .find(|v| v.name == "Order")
        .expect("Order");
    let person = m
        .vertex_tables
        .iter()
        .find(|v| v.name == "Person")
        .expect("Person");
    assert_eq!((person.record_count, order.record_count), (3, 3));

    // Three orders, each naming a real person: three edges, none dropped —
    // stated rather than omitted, because «no key» and «nothing dropped» are
    // not the same answer.
    let placed_by = m
        .edge_tables
        .iter()
        .find(|e| e.source.references == "Order" && e.destination.references == "Person")
        .unwrap_or_else(|| panic!("no Order→Person edge: {m:?}"));
    assert_eq!(placed_by.label, "placedBy");
    assert_eq!(placed_by.record_count, 3);
    let drops = corpus
        .report
        .dropped
        .iter()
        .find(|d| d.table == placed_by.name)
        .expect("a drop count for the edge");
    assert_eq!(drops.dropped, 0);

    // The literal `ex:amount` stayed a property on Order; `ex:placedBy` did not.
    let order_columns: Vec<&str> = order.properties.iter().map(|p| p.name.as_str()).collect();
    assert!(order_columns.contains(&"amount"), "{order_columns:?}");
    assert!(!order_columns.contains(&"placedBy"), "{order_columns:?}");

    // The type IRI, and a property's storage type.
    assert_eq!(person.iri.as_deref(), Some("https://example.org/Person"));
    let name = person
        .properties
        .iter()
        .find(|p| p.name == "name")
        .expect("name on Person");
    assert_eq!(name.data_type, "string");
}
