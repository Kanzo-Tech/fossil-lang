//! **A file the catalogue reads is not parsed as a program**, over the wire.
//!
//! This is the LSP half of `fossil-wasm`'s `tests/documents_are_not_programs.rs`
//! (`353228c`), whose own commit message closes by naming this defect as
//! uncovered: `publish_diagnostics` fed every open buffer to
//! `fossil_mir::program_diagnostics`, and nothing asked whether the buffer held
//! fossil source. An editor opens the `.shex` a program names — that is the
//! ordinary way to look at your own output contract — and got the fossil parser
//! run over it, with every complaint attributed to the document. In VS Code that
//! is squiggles down the length of the user's `ShEx`.
//!
//! The playground measured **twenty-one** rows for the `ShExJ` document below.
//! This test drives the real binary over stdio, and the number it measured
//! before the guard existed is in the assertion message where it belongs.
//!
//! # What answers «which open files are programs»
//!
//! Nothing new. `LspSystem::providers` returns
//! `fossil_descriptors_output::PROVIDERS` — the same table the playground
//! installs — every row of which declares the extensions it accepts, and
//! `fossil_base::claimed` asks all of them at once. A URI some row reads is an
//! INPUT, and an input is not a program. The extension is read off the LSP's
//! `path`, which in this host is the whole `file://…` URI; `Provider::accepts`
//! goes through `extension_of`, which takes the last path segment first, so a
//! URI answers exactly as a path does.
//!
//! # The three things this pins
//!
//! 1. A `.shex` buffer open in the editor gets a `publishDiagnostics` with an
//!    EMPTY array — the notification is still sent, because the editor has to be
//!    told to clear whatever it drew last time.
//! 2. It is the CATALOGUE deciding and not a `.shex` special case: a `.csv`
//!    buffer is silent for the same reason, and an extension no row claims is
//!    still checked — so an unrecognised file falls back to the old behaviour
//!    and never to silence.
//! 3. **Silence is not deafness.** The program that names the document is still
//!    checked against it — the report below exists only because the `ShExJ` was
//!    decoded — and a program with a mistake in it still reports.

#![cfg(not(target_arch = "wasm32"))]
// `{Users.id}` is a Fossil interpolation hole in a literal program, not a Rust
// format-string argument.
#![allow(clippy::literal_string_with_formatting_args)]

mod common;

use std::path::PathBuf;

use common::{did_open, drive, notif, req};

/// The program, naming its output document and writing one property the
/// document does not declare.
///
/// `nickname` is what makes point 3 checkable: «the target shape declares no
/// `nickname`» can only be said by a checker that decoded the `ShExJ` and knows
/// which predicates are in it.
const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
Users := io.csv(\"users.csv\")
People : Person from Users
    @subject = \"http://example.org/u/{Users.id}\"
    name = Users.name
    nickname = Users.nick
";

/// `fossil-wasm`'s `DEMANDS_INTEGER`, verbatim — the document the twenty-one
/// rows were measured on. A different one would be a different count.
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

/// Not fossil source, byte for byte — so anything reported about it is about
/// the file's NAME and nothing else.
const NOT_FOSSIL: &str = "id,name\n1,Ada\n";

/// A fresh directory for one test, unique per process.
///
/// The pid is load-bearing for the same reason it is in
/// `fossil-cli/tests/common/mod.rs`: two concurrent `cargo test` runs over one
/// checkout otherwise share the path and delete each other's fixtures mid-run.
fn workdir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fossil-lsp-documents-{test_name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create workdir");
    dir
}

/// `file://<dir>/<name>` — the URI the editor opens a file under, which is also
/// the key the server registers it in the file registry under (`main.rs`'s
/// `LspState::open`).
fn uri(dir: &std::path::Path, name: &str) -> String {
    format!("file://{}", dir.join(name).display())
}

/// The one `publishDiagnostics` array for `uri`, or a panic — the server sends
/// exactly one per `didOpen`, and none at all means the buffer never opened.
fn published_for<'a>(t: &'a common::Transcript, uri: &str) -> &'a Vec<serde_json::Value> {
    let notes = t.published(uri);
    assert_eq!(
        notes.len(),
        1,
        "expected exactly one publishDiagnostics for {uri}; stderr: {}",
        t.stderr
    );
    notes[0]
        .pointer("/params/diagnostics")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "publishDiagnostics missing params.diagnostics: {}",
                notes[0]
            )
        })
}

/// (1) and (3): the `.shex` the user opened draws no squiggle, and the program
/// that names it is still checked against it.
///
/// The document is opened FIRST on purpose. `LspState::open` registers a buffer
/// under its own URI so the OPEN COPY is what every program naming it reads, and
/// silencing the document must not undo that — so the report on the program
/// below is a report against the buffer, not against the file on disk.
#[test]
fn a_shape_document_open_in_the_editor_is_not_parsed_as_fossil() {
    let dir = workdir("shex");
    std::fs::write(dir.join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.join("person.shex"), DEMANDS_INTEGER).expect("write document");
    let prog_uri = uri(&dir, "prog.fossil");
    let doc_uri = uri(&dir, "person.shex");

    let t = drive(&[
        req(
            1,
            "initialize",
            serde_json::json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", serde_json::json!({})),
        did_open(&doc_uri, DEMANDS_INTEGER),
        did_open(&prog_uri, PROGRAM),
        req(2, "shutdown", serde_json::Value::Null),
        notif("exit", serde_json::Value::Null),
    ]);

    // (1) The editor is told about the document — and told there is nothing
    // wrong with it. Twenty-one rows before the guard in `diagnostics_for`.
    let in_document = published_for(&t, &doc_uri);
    assert!(
        in_document.is_empty(),
        "a `ShExJ` document is not fossil source and its parse errors are not \
         about it; got {} diagnostic(s): {in_document:#?}",
        in_document.len()
    );

    // (3) And the document is still the OUTPUT CONTRACT the program is checked
    // against: `nickname` is a property no predicate in the `ShExJ` ends in, and
    // only a checker that decoded the document can say so.
    let in_program = published_for(&t, &prog_uri);
    let messages: Vec<&str> = in_program
        .iter()
        .filter_map(|d| d.get("message").and_then(serde_json::Value::as_str))
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("declares no `nickname`")),
        "the program is still checked against the document it names; got \
         {messages:#?}"
    );
}

/// (2): the rule is the catalogue's, not a `.shex` special case. A `.csv` is
/// read by `io.csv` and is silent for exactly the same reason; an extension no
/// row claims is checked, so the failure mode of an unrecognised file is the old
/// behaviour and never silence.
#[test]
fn the_catalogue_is_what_decides_and_not_a_shex_special_case() {
    let dir = workdir("catalogue");
    let csv_uri = uri(&dir, "users.csv");
    let unknown_uri = uri(&dir, "users.unknown");

    let t = drive(&[
        req(
            1,
            "initialize",
            serde_json::json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", serde_json::json!({})),
        did_open(&csv_uri, NOT_FOSSIL),
        did_open(&unknown_uri, NOT_FOSSIL),
        req(2, "shutdown", serde_json::Value::Null),
        notif("exit", serde_json::Value::Null),
    ]);

    assert!(
        published_for(&t, &csv_uri).is_empty(),
        "`io.csv` reads `.csv`, so a `.csv` buffer is an input, not a program"
    );
    assert!(
        !published_for(&t, &unknown_uri).is_empty(),
        "no row claims `.unknown`, so it is checked — an unrecognised file falls \
         back to the old behaviour, never to silence"
    );
}

/// (3), the other half: a program with a mistake in it still reports, so the
/// guard cannot be passing by turning the drain off.
#[test]
fn a_program_with_a_mistake_still_reports() {
    let dir = workdir("broken");
    let broken_uri = uri(&dir, "broken.fossil");

    let t = drive(&[
        req(
            1,
            "initialize",
            serde_json::json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", serde_json::json!({})),
        did_open(
            &broken_uri,
            "User : Nowhere from nothing\n    name = nothing.name\n",
        ),
        req(2, "shutdown", serde_json::Value::Null),
        notif("exit", serde_json::Value::Null),
    ]);

    assert!(
        !published_for(&t, &broken_uri).is_empty(),
        "a `.fossil` buffer is a program and is still checked"
    );
}
