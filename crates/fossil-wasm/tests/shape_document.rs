//! The editor half of the shape-document seam: the OPEN BUFFER is the
//! document, and editing it re-checks the programs that name it.
//!
//! This is the browser-side twin of `fossil-engine`'s
//! `documents::tests::editing_the_document_rechecks_the_program_and_the_diagnostic_changes`.
//! There the document is registered from a filesystem; here there is no
//! filesystem at all, so the only way a document reaches the compiler is
//! `open_file`, and the only way it changes is `update_file` — which is
//! `set_text` on the very Salsa input the checker reads.
//!
//! What it proves, in order:
//!
//!   1. A program whose document nobody opened resolves NO output contract.
//!      That is not a silent failure: the playground has no disk, and the
//!      registry says "not there" rather than caching a wrong answer.
//!   2. Opening the document afterwards invalidates the check that missed it —
//!      the diagnostic appears without anyone touching the program.
//!   3. Editing the document's text changes the diagnostic. This is what the
//!      whole change was for; `System::read_file` could not see the edit
//!      because nothing depended on it.
//!   4. Editing the PROGRAM does not change what it is checked against.

#![cfg(not(target_arch = "wasm32"))]
// `${ex:}` / `${.id}` are Fossil template placeholders in a literal program,
// not Rust format-string arguments.
#![allow(clippy::literal_string_with_formatting_args)]

use fossil_wasm::FossilPlayground;

/// A program that names its output shape document and writes `name` from a CSV
/// column.
///
/// The key is the bare name — the last segment of the predicate
/// IRI `http://example.org/name` the shape declares — and the identity is the
/// `@subject` slot, first line of the body.
const PROGRAM: &str = "\
prefix ex: <http://example.org/>
type { Person } = io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
User : ex:Person from users
    @subject = `${ex:}u/${.id}`
    name = .name
";

/// `http://example.org/name` narrowed to `xsd:integer`.
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

/// The same document with the constraint the program satisfies.
const DEMANDS_STRING: &str = r#"{
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
            "datatype": "http://www.w3.org/2001/XMLSchema#string"
          }
        }
      }
    }
  ]
}"#;

/// The descriptor the browser would have pushed after a `DESCRIBE` — without a
/// source row `.name` has no type and there is nothing for the shape to
/// disagree with.
const USERS_DESCRIPTOR: &str = r#"{
  "uri": "users.csv",
  "columns": [
    { "name": "id", "primitive": "string" },
    { "name": "name", "primitive": "string" }
  ],
  "freshness_token": ""
}"#;

/// Every diagnostic message the workspace reports, across all open files.
fn messages(pg: &FossilPlayground) -> Vec<String> {
    pg.check_rows().into_iter().map(|r| r.message).collect()
}

fn mentions_integer(messages: &[String]) -> bool {
    messages.iter().any(|m| m.contains("expected `Integer`"))
}

#[test]
fn opening_and_editing_the_document_re_checks_the_program_that_names_it() {
    let mut pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("the descriptor JSON is well-formed");

    let program = pg.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());

    // (1) Nobody opened the document, and the playground has no disk to read
    //     it from. The program is checked against no output contract.
    let before = messages(&pg);
    assert!(
        !mentions_integer(&before),
        "with no document registered there is no contract to violate; got {before:?}"
    );

    // (2) Opening the document registers it under its own path — the key the
    //     program's `io.shex(\"person.shex\")` resolves to — and the check that
    //     missed re-runs. Nothing touched the program.
    let document = pg.open_file_native("person.shex".to_string(), DEMANDS_INTEGER.to_string());
    let with_document = messages(&pg);
    assert!(
        mentions_integer(&with_document),
        "registering the document must invalidate the check that missed it, and \
         the shape demands an integer where the program writes a string; got \
         {with_document:?}"
    );

    // (3) The edit the disk could not express: the buffer changes, and the
    //     diagnostic derived from it changes with it.
    pg.update_file_native(document, DEMANDS_STRING.to_string())
        .expect("update_file_native");
    let after_edit = messages(&pg);
    assert!(
        !mentions_integer(&after_edit),
        "editing the document must re-check every program that reads it; got \
         {after_edit:?}"
    );

    // (4) And the converse: editing the PROGRAM does not move the shape it is
    //     checked against.
    pg.update_file_native(program, PROGRAM.replace("u/${.id}", "v/${.id}"))
        .expect("update_file_native");
    assert_eq!(
        messages(&pg),
        after_edit,
        "a program edit re-checks the program against the SAME document"
    );
}

/// A document the user has open is the truth even before it is saved — there is
/// no other copy here, which is the browser stating plainly what the LSP also
/// does: the buffer wins over the disk.
#[test]
fn a_document_opened_after_the_program_is_still_found_by_it() {
    let mut pg = FossilPlayground::new();
    let _program = pg.open_file_native("a/prog.fossil".to_string(), PROGRAM.to_string());
    // Opened under the path the program's relative reference resolves to.
    let _document = pg.open_file_native("a/person.shex".to_string(), DEMANDS_INTEGER.to_string());
    pg.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("descriptor");

    assert!(
        mentions_integer(&messages(&pg)),
        "the document is resolved relative to the program that names it"
    );
}
