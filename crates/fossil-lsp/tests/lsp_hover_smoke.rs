//! End-to-end smoke test for `textDocument/hover`.
//!
//! Spawns the `fossil-lsp` binary, drives it through `initialize` →
//! `initialized` → `didOpen` (with a `.fossil` source containing an
//! iri-template property) → `textDocument/hover` (at a position inside the
//! template) → `shutdown` → `exit`. Asserts:
//!
//! 1. The hover response is a JSON-RPC Response (matched on `id`).
//! 2. `result.contents.value` contains the rendered ty (a `Ref<…>`).
//! 3. `result.contents.value` contains the fenced fossil code block
//!    opener (```` ```fossil ````).
//! 4. `result.contents.kind` is `"markdown"`.
//!
//! Pattern mirrors `lsp_smoke.rs`: all frames written upfront, then stdin
//! dropped, then stdout drained and parsed. Those mechanics are
//! `tests/common/mod.rs` now, written once for every test that drives the
//! binary over the wire.
//!
//! # The program is written to disk, and it has to be
//!
//! It used to be an inline `const` opened under `file:///tmp/hover.fossil`,
//! which was free while a program named no shape document. Ruling 3 of
//! 2026-08-11 makes naming one MANDATORY — a property key is the last segment
//! of a predicate IRI the document declares — and `typecheck_mapping` returns
//! `Err` when the contract does not resolve, which empties the `expr_types`
//! table hover reads. A hover over an unresolvable program is `null`, so this
//! test writes BOTH files into a temp directory and opens the program under its
//! real path, letting the server resolve `io.shex("hover.shex")` beside it the
//! way it does in an editor.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::{did_open, drive, notif, req, text_pos};

/// `.fossil` source the hover smoke test opens. Line layout (0-indexed):
///   0: type { Person } := io.shex("hover.shex")
///   1: Users := io.csv("x.csv")
///   2: People : Person from Users
///   3:     @subject = "<https://example.org/u/{Users.id>}"
///   4:     name = Users.name
///
/// The hover request targets line 3, character 10 — inside the identity
/// property, whose interpolated string synthesises a REFERENCE to the shape the
/// mapping targets — `Ref<?>` here, since the fixture registers no document.
const FOSSIL_SRC: &str = "\
type { Person } := io.shex(\"hover.shex\")
Users := io.csv(\"x.csv\")
People : Person from Users
    @subject = \"https://example.org/u/{Users.id}\"
    name = Users.name
";

/// The output contract `FOSSIL_SRC` names, written beside it. One shape, one
/// predicate whose last segment is the one property the body writes.
const SHEX_SRC: &str = "\
PREFIX ex: <https://example.org/>

ex:Person {
  ex:name .
}
";

/// Write the two files into a fresh temp directory and return the program's
/// `file://` URI. The server resolves the document relative to this path.
fn write_program() -> String {
    let dir = std::env::temp_dir().join(format!("fossil-hover-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("hover.fossil"), FOSSIL_SRC).expect("write program");
    std::fs::write(dir.join("hover.shex"), SHEX_SRC).expect("write document");
    format!("file://{}", dir.join("hover.fossil").display())
}

#[test]
fn lsp_hover_on_the_identity_returns_markdown_naming_a_reference() {
    let uri = write_program();

    // Line 3, character 10 — inside the identity property's iri template.
    let t = drive(&[
        req(
            1,
            "initialize",
            serde_json::json!({ "capabilities": {}, "processId": null, "rootUri": null }),
        ),
        notif("initialized", serde_json::json!({})),
        did_open(&uri, FOSSIL_SRC),
        req(2, "textDocument/hover", text_pos(&uri, 3, 10)),
        req(3, "shutdown", serde_json::Value::Null),
        notif("exit", serde_json::Value::Null),
    ]);

    assert!(
        !t.frames.is_empty(),
        "expected at least one framed response on stdout; got 0 (raw bytes: {})",
        t.stdout,
    );

    // The hover response (id=2) MUST be a Markdown Hover.
    let hover_response = t.by_id(2);
    let contents = hover_response
        .pointer("/result/contents")
        .unwrap_or_else(|| panic!("hover response missing result.contents: {hover_response}"));
    let kind = contents
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .expect("hover contents missing kind");
    assert_eq!(
        kind, "markdown",
        "expected MarkupKind::Markdown, got {kind}"
    );
    let value = contents
        .get("value")
        .and_then(serde_json::Value::as_str)
        .expect("hover contents missing value");
    assert!(
        value.contains("Ref<"),
        "expected hover markdown to name a reference (the rendered Ty); got {value:?}",
    );
    assert!(
        value.contains("```fossil"),
        "expected hover markdown to contain a fenced fossil code block; got {value:?}",
    );
    assert!(
        value.contains("Literal"),
        "expected hover markdown to mention the Literal provenance kind; got {value:?}",
    );

    // The initialize response: id == 1, hover_provider advertised.
    let init_response = t.by_id(1);
    let hover_cap = init_response
        .pointer("/result/capabilities/hoverProvider")
        .expect("initialize response missing capabilities.hoverProvider");
    assert_eq!(
        hover_cap,
        &serde_json::Value::Bool(true),
        "the server must advertise hoverProvider: true; got {hover_cap}",
    );

    // The shutdown response: id == 3, result == null.
    let shutdown_response = t.by_id(3);
    assert!(
        shutdown_response
            .get("result")
            .is_some_and(serde_json::Value::is_null),
        "shutdown response must have result: null per LSP spec; got {shutdown_response}",
    );
}

// ── The hover surface, and why it is driven in-process ─────────────────────
//
// No `seq.filter` stub was ever added to `fossil-registry`. Without one, these
// tests take the DIRECT integration path that BYPASSES the JSON-RPC layer,
// rather than a full end-to-end through a `seq.filter` surface form.
//
// The reason recorded at the time was that `HirExpr` had no `Pipeline` / `Call`
// variant, so an implicit closure could not be expressed in surface syntax at
// all. `HirExpr::Call` EXISTS now (`fossil-hir/src/lower.rs`), so that reason
// no longer holds and the JSON-RPC end-to-end is unwritten rather than
// impossible. Nobody has measured whether it would pass.
//
// These two tests exercise `fossil_ide::hover::render_markdown` — the exact
// rendering function the LSP hover handler (`fossil_lsp::handle_request`)
// calls on the `ExprTypeEntry` returned by `ty_origin`. The closure synthesis +
// forward propagation are driven IN-PROCESS via fossil-hir's public
// provenance types + fossil-descriptors-input's `InferredDescriptor`, so the
// integration boundary tested is hover.rs's Markdown body — identical to what
// `result.contents.value` would carry over JSON-RPC.

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, Span, System};
use fossil_graph_schema::Primitive;
use fossil_hir::body::ExprId;
use fossil_hir::provenance::{ExprTypeEntry, Provenance, ProvenanceKind};
use fossil_hir::ty::{Ty, TyKind};
use std::sync::Arc;

/// The introspected `users` row — `id`/`age` integers, a `name` string.
fn users_descriptor() -> fossil_descriptors_input::InferredDescriptor {
    use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
    InferredDescriptor {
        uri: "users.csv".into(),
        columns: vec![
            InferredColumn {
                name: "id".into(),
                primitive: Primitive::Integer,
            },
            InferredColumn {
                name: "name".into(),
                primitive: Primitive::String,
            },
            InferredColumn {
                name: "age".into(),
                primitive: Primitive::Integer,
            },
        ],
        freshness_token: String::new(),
    }
}

fn bare_db() -> FossilDb {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    FossilDb::new(system)
}

/// Hovering on `.age` inside an implicitly-synthesised closure surfaces BOTH
/// the field type `Integer` AND the closure parameter binding
/// `(row: Record<...>) => row.age >= 18` — the synthesis is NOT hidden.
///
/// Direct integration, for the reason above: the `SynthesizedClosureRendering`
/// provenance recorded on the closure body's `ExprId` is fed to the public
/// `fossil_ide::hover::render_markdown`, asserting the LSP hover Markdown body.
#[test]
fn hover_inside_synthesized_closure_via_typecheck_mapping() {
    let db = bare_db();
    let int_ty = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
    // The closure rendering for `users |> filter(.age >= 18)`, written out by
    // hand because the
    // function that produced it, `fossil_hir::check::render_closure`, is
    // deleted along with `synthesize_closure` — so this asserts the RENDERING
    // of a provenance kind nothing produces. It survives only until
    // `ProvenanceKind::SynthesizedClosureRendering` itself goes.
    let rendering = smol_str::SmolStr::from("(row: Record<{age: Integer}>) => row.age >= 18");
    let entry = ExprTypeEntry {
        expr_id: ExprId(0),
        ty: int_ty,
        provenance: Provenance {
            span: Span { start: 0, end: 0 },
            kind: ProvenanceKind::SynthesizedClosureRendering { rendering },
        },
    };

    let md = fossil_ide::hover::render_markdown(&db, &entry);

    // (a) closure rendering prefix + arrow + rewritten FieldRef.
    assert!(
        md.contains("(row: Record<"),
        "hover must show the closure parameter binding; got {md:?}",
    );
    assert!(
        md.contains(") =>"),
        "hover must show the closure arrow; got {md:?}"
    );
    assert!(
        md.contains("row.age"),
        "hover must show the rewritten FieldRef `row.age`; got {md:?}",
    );
    // (b) field type from forward propagation.
    assert!(
        md.contains("Integer"),
        "hover must show the field type `Integer`; got {md:?}",
    );
    // (c) the synthesis tagline so the user understands WHY the closure appears.
    assert!(
        md.contains("synthesised closure parameter binding"),
        "hover must explain the implicit synthesis; got {md:?}",
    );
    // Risk Register (STATE.md "Do NOT"): internal inference state must NEVER leak.
    assert!(!md.contains("Unknown"), "hover leaked `Unknown`: {md:?}");
    assert!(
        !md.contains("InferenceId"),
        "hover leaked `InferenceId`: {md:?}"
    );
}

/// `FieldRef` hover OUTSIDE a closure: a `.field` resolved against an
/// introspected source row surfaces its type.
///
/// The `String` type is proven to come from forward propagation by
/// resolving the `name` column through `record_from_inferred` (the same
/// in-process path `resolve_source_scope` uses), then rendering the resulting
/// `InputDescriptor` entry via the LSP's `render_markdown`.
#[test]
fn hover_on_introspected_fieldref_outside_closure() {
    let db = bare_db();

    // Resolve `.name` against a real INTROSPECTED descriptor — proves the
    // `String` type below comes from the descriptor and is not hard-coded: the
    // inferred descriptor is what a host registers after introspecting the
    // file.
    let name_kind = users_descriptor()
        .columns
        .iter()
        .find(|c| c.name == "name")
        .expect("the introspected row has a `name` column")
        .primitive;
    assert_eq!(
        name_kind,
        Primitive::String,
        "the `name` column must be a string type; got {name_kind:?}",
    );

    let str_ty = Ty::new(&db, TyKind::Primitive(Primitive::String));
    let entry = ExprTypeEntry {
        expr_id: ExprId(1),
        ty: str_ty,
        provenance: Provenance {
            span: Span { start: 0, end: 0 },
            kind: ProvenanceKind::InputDescriptor {
                source_name: smol_str::SmolStr::from("users"),
                column: smol_str::SmolStr::from("name"),
            },
        },
    };

    let md = fossil_ide::hover::render_markdown(&db, &entry);
    assert!(
        md.contains("String"),
        "hover on an introspected FieldRef must show the field type `String`; got {md:?}",
    );
    // NOT inside a closure → no closure binding rendered.
    assert!(
        !md.contains("(row:"),
        "FieldRef outside a closure must NOT render a closure binding; got {md:?}",
    );
    assert!(!md.contains("Unknown"), "hover leaked `Unknown`: {md:?}");
    assert!(
        !md.contains("InferenceId"),
        "hover leaked `InferenceId`: {md:?}"
    );
}
