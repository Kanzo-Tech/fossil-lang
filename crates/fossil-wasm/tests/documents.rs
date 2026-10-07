//! Fossil resolves, the host reads: the workspace reports the documents a
//! program is missing and the sources it reads, and takes back only text.
//!
//! `resolveDocuments` in `@fossil-lang/types` is the loop over these three
//! calls; this is the Rust half it drives.

// `{users.id}` is a Fossil interpolation hole in a literal program, not a Rust
// format-string argument.
#![allow(clippy::literal_string_with_formatting_args)]

use std::collections::HashMap;

use fossil_hir::documents::MissingDocument;
use fossil_lineage::{Input, Role};
use fossil_wasm::FossilWorkspace;

const PROGRAM: &str = "\
type { Person } := io.shex(\"@vocab/person.shex\")
users := io.csv(\"@lake/users.csv\", delimiter = \"|\")
User : Person from users
    @subject = \"http://example.org/u/{users.id}\"
    name = users.name
";

/// `http://example.org/name` narrowed to `xsd:integer`, where the program
/// writes a string — so a resolved document is visible as a diagnostic.
const DEMANDS_INTEGER: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/name",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#integer"
          }
        }
      }
    }
  ]
}"#;

const USERS_DESCRIPTOR: &str = r#"{
  "key": "@lake/users.csv",
  "columns": [
    { "name": "id", "primitive": "string" },
    { "name": "name", "primitive": "string" }
  ],
  "etag": ""
}"#;

fn connections() -> HashMap<String, String> {
    HashMap::from([
        (
            "vocab".to_string(),
            "https://minio.example/shapes".to_string(),
        ),
        ("lake".to_string(), "s3://lake/".to_string()),
    ])
}

fn violates_contract(ws: &FossilWorkspace) -> bool {
    ws.diagnostic_rows()
        .iter()
        .any(|r| r.code == "type/property-mismatch" && r.detail.data["expected"] == "Integer")
}

#[test]
fn registering_what_is_missing_under_its_key_is_what_the_checker_reads() {
    let mut ws = FossilWorkspace::new();
    ws.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("descriptor");
    ws.set_connections_native(connections());
    let program = ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());

    let missing = ws.missing_documents_native(program).expect("open handle");
    assert_eq!(
        missing,
        [MissingDocument {
            key: "@vocab/person.shex".to_string(),
            location: "https://minio.example/shapes/person.shex".to_string(),
            connection: Some("vocab".to_string()),
        }]
    );
    assert!(!violates_contract(&ws), "nothing registered, no contract");

    ws.register_document_native(&missing[0].key, DEMANDS_INTEGER);
    assert!(
        ws.missing_documents_native(program)
            .expect("open")
            .is_empty()
    );
    assert!(
        violates_contract(&ws),
        "the registered document is the program's output contract"
    );
}

/// The map reaches locations and never keys, so repointing a connection leaves
/// a registered document registered.
#[test]
fn a_connection_map_moves_the_location_and_not_the_key() {
    let mut ws = FossilWorkspace::new();
    let program = ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    let unmapped = ws.missing_documents_native(program).expect("open");
    assert_eq!(unmapped[0].location, "@vocab/person.shex");

    ws.set_connections_native(connections());
    let mapped = ws.missing_documents_native(program).expect("open");
    assert_eq!(mapped[0].key, unmapped[0].key);

    ws.register_document_native(&mapped[0].key, DEMANDS_INTEGER);
    ws.set_connections_native(HashMap::new());
    assert!(
        ws.missing_documents_native(program)
            .expect("open")
            .is_empty()
    );
}

/// Opening a file registers the buffer and nothing it names: a document
/// arrives only through `register_document`, or as a buffer being edited.
#[test]
fn opening_a_program_registers_no_document_and_an_open_buffer_is_not_missing() {
    let mut ws = FossilWorkspace::new();
    let program = ws.open_file_native(
        "prog.fossil".to_string(),
        PROGRAM.replace("@vocab/person.shex", "person.shex"),
    );
    assert_eq!(ws.missing_documents_native(program).expect("open").len(), 1);

    ws.open_file_native("person.shex".to_string(), DEMANDS_INTEGER.to_string());
    assert!(
        ws.missing_documents_native(program)
            .expect("open")
            .is_empty()
    );
}

#[test]
fn inputs_are_keyed_as_written_and_located_through_the_map() {
    let mut ws = FossilWorkspace::new();
    ws.set_connections_native(connections());
    let program = ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    let inputs = ws.inputs_native(program).expect("open handle");
    assert_eq!(
        inputs
            .iter()
            .filter(|i| i.role == Role::Data)
            .collect::<Vec<_>>(),
        [&Input {
            role: Role::Data,
            binding: Some("users".to_string()),
            key: "@lake/users.csv".to_string(),
            location: "s3://lake/users.csv".to_string(),
            connection: Some("lake".to_string()),
            format: Some("csv".to_string()),
            option: Some("|".to_string()),
        }]
    );
    let schema: Vec<&str> = inputs
        .iter()
        .filter(|i| i.role == Role::Schema)
        .map(|i| i.key.as_str())
        .collect();
    assert_eq!(schema, ["@vocab/person.shex"]);
}

#[test]
fn an_unknown_handle_has_no_documents_and_no_inputs() {
    let mut ws = FossilWorkspace::new();
    let handle = ws.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    ws.close_file_native(handle).expect("open");
    assert!(ws.missing_documents_native(handle).is_none());
    assert!(ws.inputs_native(handle).is_none());
}
