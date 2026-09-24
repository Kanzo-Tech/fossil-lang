//! Fossil resolves, the host reads: the playground reports the documents a
//! program is missing and the sources it reads, and takes back only text.
//!
//! `resolveDocuments` in `@fossil-lang/types` is the loop over these three
//! calls; this is the Rust half it drives.

#![cfg(not(target_arch = "wasm32"))]
// `{users.id}` is a Fossil interpolation hole in a literal program, not a Rust
// format-string argument.
#![allow(clippy::literal_string_with_formatting_args)]

use std::collections::HashMap;

use fossil_lineage::ProgramSource;
use fossil_wasm::{FossilPlayground, MissingDocumentRow};

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
  "uri": "@lake/users.csv",
  "columns": [
    { "name": "id", "primitive": "string" },
    { "name": "name", "primitive": "string" }
  ],
  "freshness_token": ""
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

fn violates_contract(pg: &FossilPlayground) -> bool {
    pg.check_rows()
        .iter()
        .any(|r| r.message.contains("expects Integer"))
}

#[test]
fn registering_what_is_missing_under_its_key_is_what_the_checker_reads() {
    let mut pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("descriptor");
    pg.set_connections_native(connections());
    let program = pg.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());

    let missing = pg.missing_documents_native(program).expect("open handle");
    assert_eq!(
        missing,
        [MissingDocumentRow {
            key: "@vocab/person.shex".to_string(),
            locator: "https://minio.example/shapes/person.shex".to_string(),
        }]
    );
    assert!(!violates_contract(&pg), "nothing registered, no contract");

    pg.register_document_native(&missing[0].key, DEMANDS_INTEGER);
    assert!(
        pg.missing_documents_native(program)
            .expect("open")
            .is_empty()
    );
    assert!(
        violates_contract(&pg),
        "the registered document is the program's output contract"
    );
}

/// The map reaches locators and never keys, so repointing a connection leaves
/// a registered document registered.
#[test]
fn a_connection_map_moves_the_locator_and_not_the_key() {
    let mut pg = FossilPlayground::new();
    let program = pg.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    let unmapped = pg.missing_documents_native(program).expect("open");
    assert_eq!(unmapped[0].locator, "@vocab/person.shex");

    pg.set_connections_native(connections());
    let mapped = pg.missing_documents_native(program).expect("open");
    assert_eq!(mapped[0].key, unmapped[0].key);

    pg.register_document_native(&mapped[0].key, DEMANDS_INTEGER);
    pg.set_connections_native(HashMap::new());
    assert!(
        pg.missing_documents_native(program)
            .expect("open")
            .is_empty()
    );
}

/// Opening a file registers the buffer and nothing it names: a document
/// arrives only through `register_document`, or as a buffer being edited.
#[test]
fn opening_a_program_registers_no_document_and_an_open_buffer_is_not_missing() {
    let mut pg = FossilPlayground::new();
    let program = pg.open_file_native(
        "prog.fossil".to_string(),
        PROGRAM.replace("@vocab/person.shex", "person.shex"),
    );
    assert_eq!(pg.missing_documents_native(program).expect("open").len(), 1);

    pg.open_file_native("person.shex".to_string(), DEMANDS_INTEGER.to_string());
    assert!(
        pg.missing_documents_native(program)
            .expect("open")
            .is_empty()
    );
}

#[test]
fn sources_are_keyed_as_written_and_located_through_the_map() {
    let mut pg = FossilPlayground::new();
    pg.set_connections_native(connections());
    let program = pg.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    assert_eq!(
        pg.sources_native(program).expect("open handle"),
        [ProgramSource {
            binding: "users".to_string(),
            key: "@lake/users.csv".to_string(),
            locator: "s3://lake/users.csv".to_string(),
            format: "csv".to_string(),
            option: Some("|".to_string()),
        }]
    );
}

#[test]
fn an_unknown_handle_has_no_documents_and_no_sources() {
    let mut pg = FossilPlayground::new();
    let handle = pg.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    pg.close_file_native(handle).expect("open");
    assert!(pg.missing_documents_native(handle).is_none());
    assert!(pg.sources_native(handle).is_none());
}
