//! The editor reports what `fossil check` reports, for the diagnostics that
//! need a source column's TYPE.
//!
//! `fossil check apps/docs/programs/errors/unknown-field/program.fossil` prints
//! ``` `nmae` is not a field of `User` ``` and exits 1. Driven over stdio, the
//! LSP published **zero** diagnostics for the same bytes — measured 2026-08-23,
//! and it was not a transport fault: [`the_transport_is_not_what_is_missing`]
//! opens a program whose one diagnostic needs no descriptor and watches it
//! arrive down the same pipe.
//!
//! The cause was one absent method. `LspSystem` did not override
//! [`fossil_base::System::descriptors`], so it inherited the trait's `None`;
//! `unknown field` is produced by comparing a member access against the columns
//! a `DESCRIBE` found, and with no table to find them in the checker has nothing
//! to disagree with. The editor was silent on an entire CLASS of real errors —
//! every one whose evidence is a source's schema.
//!
//! # What the editor will and will not go and read
//!
//! `fossil check` may block on an `s3://` round trip; it is a command and the
//! user is already waiting. This server may not: `main_loop` is one sequential
//! loop over one channel, so a `didOpen` that waits on the network is not one
//! slow file, it is hover and completion dead in every other buffer too.
//!
//! So the editor introspects exactly what it can `stat` —
//! `fossil_introspect::Reach::Local`. That is a local file that exists right
//! now; a remote locator, a path that does not exist yet, and a half-typed one
//! are all the same answer, and it is the answer `freshness_token` already
//! gave. [`a_remote_source_is_not_introspected_by_the_editor`] pins the gap that
//! leaves, because it is a real one and the docblock alone would not keep it
//! visible.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::PathBuf;

use common::{did_open, drive, notif, req};
use serde_json::{Value, json};

/// A conformance program's absolute path. The program must be opened under its
/// REAL path or nothing beside it resolves — not `shop.shex`, and not
/// `data/users.csv`.
fn program_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/docs/programs/errors")
        .join(name)
        .join("program.fossil")
        .canonicalize()
        .unwrap_or_else(|e| panic!("apps/docs/programs/errors/{name}/program.fossil: {e}"))
}

fn program_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

/// Open one program, drain what the server published for it.
fn published_for(name: &str) -> Vec<Value> {
    let path = program_path(name);
    let uri = program_uri(&path);
    let text = std::fs::read_to_string(&path).expect("read the conformance program");
    let t = drive(&[
        req(
            1,
            "initialize",
            json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", json!({})),
        did_open(&uri, &text),
        req(99, "shutdown", Value::Null),
        notif("exit", Value::Null),
    ]);
    t.published(&uri)
        .last()
        .and_then(|n| n.pointer("/params/diagnostics"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the server published no diagnostics notification at all for {uri}; stderr: {}",
                t.stderr
            )
        })
}

fn messages(diagnostics: &[Value]) -> Vec<&str> {
    diagnostics
        .iter()
        .filter_map(|d| d.get("message").and_then(Value::as_str))
        .collect()
}

/// The control. `errors/two-identities` conflicts two `@subject` templates,
/// which is decided from the program alone — no source column's type is
/// consulted. It has always arrived.
///
/// Without this, a red [`a_misspelt_source_field_reaches_the_editor`] could be
/// a broken pipe, a bad URI, or a server that crashed on `didOpen`, and the
/// failure message would not say which.
#[test]
fn the_transport_is_not_what_is_missing() {
    let published = published_for("two-identities");
    assert!(
        !published.is_empty(),
        "the identity conflict must reach the editor, or this file is measuring \
         the transport and not the descriptor table: {published:#?}"
    );
}

/// The gap this file exists for: a member access naming a column the CSV does
/// not have.
///
/// `User.nmae` against a `data/users.csv` whose header is `id,email,name,age`.
/// The checker can only report it if the host went and read that header, which
/// is what `LspSystem`'s descriptor table plus the `didOpen` introspection now
/// do.
#[test]
fn a_misspelt_source_field_reaches_the_editor() {
    let published = published_for("unknown-field");
    let msgs = messages(&published);
    assert!(
        msgs.iter().any(|m| m.contains("nmae")),
        "`fossil check` reports ``nmae` is not a field of `User`` for this exact \
         file and exits 1. The editor published {} diagnostic(s), none naming \
         `nmae`: {msgs:#?}",
        msgs.len()
    );
}

/// The quick-fix rides along, and it is the half a user acts on.
///
/// The did-you-mean is a STRUCTURED field on the diagnostic, so the code action
/// is only offerable once the diagnostic exists at all. Asked over the wire, for
/// the range the diagnostic underlines.
#[test]
fn the_did_you_mean_is_offered_as_a_code_action() {
    let path = program_path("unknown-field");
    let uri = program_uri(&path);
    let text = std::fs::read_to_string(&path).expect("read the conformance program");
    // `name     = User.nmae` — the whole line, so the request's range certainly
    // overlaps the typo's span.
    let line = u32::try_from(
        text.lines()
            .position(|l| l.contains("User.nmae"))
            .expect("the fixture spells `User.nmae`"),
    )
    .expect("line fits u32");

    let t = drive(&[
        req(
            1,
            "initialize",
            json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", json!({})),
        did_open(&uri, &text),
        req(
            10,
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": line, "character": 0 },
                    "end": { "line": line, "character": 40 }
                },
                "context": { "diagnostics": [] }
            }),
        ),
        req(99, "shutdown", Value::Null),
        notif("exit", Value::Null),
    ]);
    let actions = t.by_id(10)["result"].clone();
    let titles: Vec<String> = actions
        .as_array()
        .map(|a| a.iter().map(|x| x["title"].to_string()).collect())
        .unwrap_or_default();
    assert!(
        titles.iter().any(|t| t.contains("name")),
        "the editor must offer «Replace with `name`» on the misspelt field; got {titles:?}"
    );
}

/// WHAT IS STILL MISSING, pinned so it cannot go quiet again.
///
/// A source the editor cannot `stat` is not introspected, so a typo in a column
/// of an `s3://` or `https://` CSV is still invisible in the editor and still
/// reported by `fossil check`. That is deliberate — see the module docs — and it
/// is a gap, not a design without a cost.
///
/// Asserted as the ABSENCE of the diagnostic, which is an assertion that will
/// go red the day somebody closes it. Read that failure as «this test is now
/// wrong», delete it, and say so.
#[test]
fn a_remote_source_is_not_introspected_by_the_editor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let program = "\
type { Person } := io.shex(\"person.shex\")
Users := io.csv(\"https://example.invalid/users.csv\")

Out : Person from Users
    @subject = \"https://shop.example/user/{Users.email}\"
    name = Users.nmae
";
    std::fs::write(
        dir.path().join("person.shex"),
        "PREFIX ex: <http://example.org/>\n\nex:Person {\n  ex:name .\n}\n",
    )
    .expect("write the shape document");
    let path = dir.path().join("prog.fossil");
    std::fs::write(&path, program).expect("write the program");
    let uri = program_uri(&path);

    let t = drive(&[
        req(
            1,
            "initialize",
            json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", json!({})),
        did_open(&uri, program),
        req(99, "shutdown", Value::Null),
        notif("exit", Value::Null),
    ]);
    let published: Vec<Value> = t
        .published(&uri)
        .last()
        .and_then(|n| n.pointer("/params/diagnostics"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let msgs = messages(&published);
    assert!(
        !msgs.iter().any(|m| m.contains("nmae")),
        "an `https://` source was introspected on the message loop. If that is \
         now deliberate, this test is the thing to delete — but the didOpen \
         handler is a blocking read and a network round trip in it stalls every \
         other buffer. Got: {msgs:#?}"
    );
}
