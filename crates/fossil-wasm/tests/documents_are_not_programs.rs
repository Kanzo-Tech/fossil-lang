//! **A file the catalogue reads is not parsed as a program.**
//!
//! The playground has no filesystem, so the only way to hand the compiler a
//! shape document is to open it — and until this test existed, opening one put
//! it in the workspace-wide drain as if it were fossil source. The `ShExJ`
//! document `shape_document.rs` uses produced **twenty-one rows** attributed to
//! `person.shex`: `unexpected token` eleven times, `expected IDENT, found
//! STRING` eight, `expected IDENT, found INDENT` once and `expected DEFINE,
//! found DEDENT` at the closing brace. In an editor that is squiggles down the
//! length of the user's `ShEx`. The `ShExC` document in the same file measured
//! fourteen, and three of THOSE claimed an *internal compiler error* — a shape
//! declaration parses far enough to look like a mapping header and then has no
//! HIR.
//!
//! # What answers «which open files are programs»
//!
//! Nothing new. `FossilPlayground` installs `fossil_descriptors_output::
//! PROVIDERS` (`wasm_system.rs`), every row of which declares the extensions it
//! accepts, and `fossil_base::claimed` asks all of them at once. A URI some row
//! reads is an INPUT — that is what `io.shex("person.shex")` means — and an
//! input is not a program. The host decides nothing about fossil's syntax here,
//! which is the objection the note this replaces raised against fixing it.
//!
//! # The four things this pins
//!
//! 1. A `ShExJ` document in the workspace contributes **zero** rows, and the
//!    number is exact because 21 is what it was.
//! 2. The per-file drain the LSP Worker publishes agrees — that is the one the
//!    squiggles come from.
//! 3. It is the CATALOGUE and not a `.shex` special case: a `.csv` nobody could
//!    parse either is silent for the same reason, and a file with an extension
//!    no row claims is still checked.
//! 4. **Silence is not deafness.** The program that names the document is still
//!    checked against it, and a real mistake in a real program still reports.

#![cfg(not(target_arch = "wasm32"))]
// `{users.id}` is a Fossil interpolation hole in a literal program, not a Rust
// format-string argument.
#![allow(clippy::literal_string_with_formatting_args)]

use fossil_wasm::FossilPlayground;

/// The program of `shape_document.rs`, naming its output document.
const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
User : Person from users
    @subject = \"http://example.org/u/{users.id}\"
    name = users.name
";

/// `shape_document.rs`'s `DEMANDS_INTEGER`, verbatim — this is the document the
/// twenty-one rows were measured on, and a different one would be a different
/// count.
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

/// The descriptor the browser would have pushed after a `DESCRIBE`. Without it
/// `users.name` has no type and the shape has nothing to disagree with.
const USERS_DESCRIPTOR: &str = r#"{
  "uri": "users.csv",
  "columns": [
    { "name": "id", "primitive": "string" },
    { "name": "name", "primitive": "string" }
  ],
  "freshness_token": ""
}"#;

fn rows_for(pg: &FossilPlayground, uri: &str) -> Vec<String> {
    pg.check_rows()
        .into_iter()
        .filter(|r| r.uri == uri)
        .map(|r| r.message)
        .collect()
}

/// (1) + (2): neither drain attributes a fossil parse error to a shape
/// document, and (4): the program is still checked against it.
#[test]
fn a_shape_document_in_the_workspace_is_not_parsed_as_fossil() {
    let mut pg = FossilPlayground::new();
    pg.register_inferred_descriptor_native(USERS_DESCRIPTOR)
        .expect("the descriptor JSON is well-formed");
    pg.open_file_native("prog.fossil".to_string(), PROGRAM.to_string());
    let document = pg.open_file_native("person.shex".to_string(), DEMANDS_INTEGER.to_string());

    // (1) The workspace-wide drain. Twenty-one before this line existed.
    let in_document = rows_for(&pg, "person.shex");
    assert!(
        in_document.is_empty(),
        "a `ShExJ` document is not fossil source and its parse errors are not \
         about it; got {} row(s): {in_document:#?}",
        in_document.len()
    );

    // (2) The per-file drain — the one the LSP Worker turns into
    // `publishDiagnostics` for the buffer the user is looking at.
    let per_file = pg
        .diagnostics_for_rows(document)
        .expect("the document handle is open");
    assert!(
        per_file.is_empty(),
        "the editor draws no squiggle in a file the catalogue reads; got {} \
         row(s): {per_file:#?}",
        per_file.len()
    );

    // (4) And the document is still the OUTPUT CONTRACT: it demands an integer
    // where the program writes a string, and that report is on the program.
    // Silencing the document must not deregister it.
    let in_program = rows_for(&pg, "prog.fossil");
    assert!(
        in_program.iter().any(|m| m.contains("expects Integer")),
        "the program is still checked against the document it names; got \
         {in_program:#?}"
    );
}

/// (3): the rule is the catalogue's, not a `.shex` special case. A `.csv` is
/// read by `io.csv` and is silent for exactly the same reason; an extension no
/// row claims is checked, so the failure mode of an unknown file is the old
/// behaviour and never silence.
#[test]
fn the_catalogue_is_what_decides_and_not_a_shex_special_case() {
    // Fossil source, byte for byte — so anything the drain reports is about the
    // file's NAME and nothing else.
    const NOT_FOSSIL: &str = "id,name\n1,Ada\n";

    let mut pg = FossilPlayground::new();
    pg.open_file_native("users.csv".to_string(), NOT_FOSSIL.to_string());
    pg.open_file_native("users.unknown".to_string(), NOT_FOSSIL.to_string());

    assert!(
        rows_for(&pg, "users.csv").is_empty(),
        "`io.csv` reads `.csv`, so a `.csv` buffer is an input, not a program"
    );
    assert!(
        !rows_for(&pg, "users.unknown").is_empty(),
        "no row claims `.unknown`, so it is checked — an unrecognised file \
         falls back to the old behaviour, never to silence"
    );
}

/// (4), the other half: a real program with a real mistake still reports, so
/// the guard cannot be passing by turning the drain off.
#[test]
fn a_program_with_a_mistake_still_reports() {
    let mut pg = FossilPlayground::new();
    let broken = pg.open_file_native(
        "broken.fossil".to_string(),
        "User : Nowhere from nothing\n    name = nothing.name\n".to_string(),
    );

    assert!(
        !rows_for(&pg, "broken.fossil").is_empty(),
        "the workspace drain still checks programs"
    );
    assert!(
        !pg.diagnostics_for_rows(broken)
            .expect("open handle")
            .is_empty(),
        "and so does the per-file drain"
    );
}
