//! The HARD CI gate for the `didChange` round-trip budget.
//!
//! It calls [`fossil_lsp::handle_notification`] with a real
//! `textDocument/didChange` notification over the canonical 200-line fixture,
//! against a real [`fossil_lsp::LspState`]. That is not «the same work the
//! handler performs» — it IS the handler, so the list of steps is not written
//! down here and cannot go stale: whatever the handler does on a keystroke is
//! inside the clock.
//!
//! The one thing left outside is the JSON-RPC FRAMING — the `Content-Length`
//! header and the byte-level read off stdin. The params decode is not: the
//! `Notification` is built before the clock starts and `handle_notification`
//! deserialises it, which is a cost the server really pays per keystroke.
//!
//! It runs as a plain `#[test]` in `cargo test`, so it is the CI hard gate.
//!
//! # This file used to reimplement the thing it gates
//!
//! `LspState` and every handler lived in `src/main.rs`, a binary target, and
//! each took an `&Connection`. Nothing outside the binary could name them, and
//! a handler needing a socket could not have been called anyway, so this file
//! kept a private `round_trip` performing what it believed the handler
//! performed. That copy went out of date twice without going red:
//!
//! - It was a hand-rolled `def_map` + `typecheck_mapping` loop while the handler
//!   drained `lower_to_mir_pg`, so every lowering a keystroke pays for was
//!   outside the clock. Repaired by editing the copy.
//! - `LspState::change` grew a source-introspection call before the `Setter`,
//!   and the new per-keystroke step was outside the clock the moment it was
//!   added. Repaired by editing the copy again.
//!
//! Two copies patched twice is the argument that a budget which executes a
//! DESCRIPTION of the handler is not a budget on the handler. `fossil-lsp`'s lib
//! target and the response-returning seam
//! (`handle_notification` returns the `publishDiagnostics` notifications rather
//! than sending them) are what let this call the real one.
//!
//! **What the copy was missing, measured.** Both were run in the same process,
//! alternating, three times, on 2026-08-25 (debug build, Apple silicon): the
//! copy averaged **3.03 ms** per keystroke and the handler **3.45 ms**, worst
//! case 3.37 ms against 3.82 ms. The copy was therefore about **14 % short**,
//! and the extra is not a regression — it is work the old number never counted:
//! `register_named_documents` on every keystroke, `fossil_ide::lsp_diagnostics`
//! (the drain, the `claimed` guard, the `LineIndex` and the UTF-16 rendering)
//! where the copy called `fossil_mir::program_diagnostics` and stopped, and the
//! JSON in and out. Against a 400 ms gate neither number is close to failing;
//! the point is which of them is a statement about `didChange`.
//!
//! **And the gate responds to the handler, which is the property being claimed.**
//! Same session, same fixture: one line added to `LspState::change` — a
//! `fossil_ide::semantic_tokens` call, a step the handler did not have — moved
//! this test from 3.45 ms to **4.65 ms** average and 3.82 ms to **5.20 ms**
//! worst, while the retired copy, run beside it in the same process, did not
//! move at all (3.07 ms → 3.15 ms, inside its own run-to-run spread). A gate
//! nobody has watched respond to the thing it gates is a gate nobody has tested.
//!
//! # Why a MARGINED budget, not a naked `< 100ms`
//!
//! The design GOAL is `< 100ms` on dev hardware, but a naked
//! `< 100ms` assertion FLAKES on shared CI runners (cold caches, neighbour
//! noise, slow debug builds). The hard gate therefore asserts a GENEROUS
//! margin ([`BUDGET_MS`]) that still catches algorithmic blow-up (an O(n²)
//! regression on a 200-line file would blow well past it) without flaking. The
//! tight `< 100ms` goal + the 20%-regression check live in the ADVISORY
//! Criterion benchmark (`benches/lsp_didchange.rs`), which calls the same
//! function.
//!
//! # The host is the editor's, because it IS the editor's
//!
//! This built its db on `fossil_base::test_support::NativeSystem`, whose provider table is the
//! trait default: **no row reads types**. Since ruling 3 of 2026-08-11 a program
//! must name a shape document, and the fixture does; under `NativeSystem` that
//! document decodes to nothing, `resolve_target_shape` fails per mapping, and
//! the checker leaves by the shortest path it has. The number that came out was
//! real and measured the wrong program.
//!
//! It was then a hand-written `EditorSystem` that copied `LspSystem`'s rows.
//! `LspState::new()` builds the real one, so there is nothing left to copy.
//! What the fixture buys is still stated as an assertion rather than a comment:
//! [`RESOLVED_PREDICATES`] is checked before the clock starts, so a future edit
//! that quietly stops resolving the contract fails here instead of producing a
//! flattering millisecond count.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::str::FromStr as _;
use std::time::{Duration, Instant};

use fossil_lsp::LspState;
use lsp_server::Notification;
use lsp_types::Uri;
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as NotificationTrait,
};
use serde_json::json;

/// The margined hard-gate budget. The design goal is < 100ms on dev hardware;
/// this 400ms ceiling absorbs CI noise + debug-build overhead while still
/// catching an algorithmic regression (which would be seconds, not ms) on a
/// 200-line file. Tightening this is the Criterion benchmark's job (advisory).
const BUDGET_MS: u128 = 400;

/// Warm-up + measured iterations. The first analysis primes Salsa's caches; we
/// then measure steady-state per-`didChange` cost (the realistic editor case is
/// an already-warm db being re-analysed after an edit).
const WARMUP: u32 = 2;
const ITERS: u32 = 10;

/// What the fixture's 15 mappings resolve between them — 60 properties, plus
/// the 15 `@subject` lines, which are not predicates.
///
/// This is the guard on the paragraph above: it is the count that goes to zero
/// the moment the host stops reading types or the document stops resolving, and
/// it is checked before anything is timed.
const RESOLVED_PREDICATES: usize = 60;

/// The path the program is opened under. It has to be the REAL one: the shape
/// document is resolved relative to the program, so a synthetic path resolves
/// nothing and silently measures a program with no output contract.
///
/// Canonicalised, because the URI below is built out of it and a `..` component
/// in a `file://` URI is a different registry key from the one the document
/// would be read back under.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/canonical_200.fossil")
        .canonicalize()
        .expect("canonicalise canonical_200.fossil")
}

/// The fixture as an editor would name it. The server keys buffers by URI and
/// derives the program's directory from it, so a bare path would exercise a
/// branch of `local_path` no editor takes.
fn fixture_uri() -> Uri {
    Uri::from_str(&format!("file://{}", fixture_path().display())).expect("a file:// URI")
}

fn fixture() -> String {
    std::fs::read_to_string(fixture_path()).expect("read canonical_200.fossil")
}

/// The `textDocument/didOpen` an editor sends when the file is first shown.
fn did_open(uri: &Uri, text: &str) -> Notification {
    Notification {
        method: DidOpenTextDocument::METHOD.to_string(),
        params: json!({
            "textDocument": {
                "uri": uri.as_str(), "languageId": "fossil", "version": 1, "text": text
            }
        }),
    }
}

/// The `textDocument/didChange` an editor sends per keystroke under
/// `TextDocumentSyncKind::FULL` — the whole buffer, every time.
///
/// Built by the caller BEFORE the clock starts. Serialising the new text into
/// the params is the client's cost, not the server's; deserialising it back out
/// is the server's, and that half is inside [`fossil_lsp::handle_notification`].
fn did_change(uri: &Uri, version: i64, text: &str) -> Notification {
    Notification {
        method: DidChangeTextDocument::METHOD.to_string(),
        params: json!({
            "textDocument": { "uri": uri.as_str(), "version": version },
            "contentChanges": [ { "text": text } ]
        }),
    }
}

/// One `didChange` round-trip: the handler, called. Returns the number of
/// diagnostics it published, so the optimiser cannot elide the work.
fn round_trip(state: &mut LspState, notif: Notification) -> usize {
    let published = fossil_lsp::handle_notification(state, notif).expect("didChange params decode");
    published
        .iter()
        .filter_map(|n| n.params.pointer("/diagnostics")?.as_array())
        .map(Vec::len)
        .sum()
}

#[test]
fn didchange_round_trip_under_margined_budget() {
    let base = fixture();
    let uri = fixture_uri();

    let mut state = LspState::new();
    // `didOpen` is the handler too: it interns the buffer, introspects the
    // sources it can `stat`, and registers the shape documents it names off
    // disk. Skipping that registration is what made an earlier number cheap.
    let opened = fossil_lsp::handle_notification(&mut state, did_open(&uri, &base))
        .expect("didOpen params decode");
    assert_eq!(
        opened.len(),
        1,
        "didOpen publishes exactly one notification"
    );
    let file = state.get(&uri).expect("the buffer is open after didOpen");

    // The contract actually resolved — assert it BEFORE timing, so a budget that
    // stops measuring the checking path fails loudly instead of getting faster.
    let db = state.db();
    let mapping_count = fossil_hir::def_map::def_map(db, file).mappings(db).len();
    let resolved: usize = fossil_hir::def_map::def_map(db, file)
        .mappings(db)
        .iter()
        .filter_map(|m| fossil_hir::check::typecheck_mapping(db, *m).ok())
        .map(|out| out.predicates(db).len())
        .sum();
    assert_eq!(
        resolved, RESOLVED_PREDICATES,
        "the fixture must resolve its output contract or this budget measures a \
         program that checks nothing; {mapping_count} mappings resolved {resolved} \
         predicates"
    );

    // Each "keystroke" appends a comment char to a comment line — a VALID-syntax
    // edit. This changes the text (so `set_text` genuinely bumps the revision)
    // without introducing a parse error or an unlexable byte, which is what
    // keeps the budget measuring the resolving path rather than the error one.
    // (A bare `#` used to hang the parser. It does not any more — see
    // `fossil-syntax/tests/recovery.rs` — but an unlexable byte still measures
    // the wrong path.)
    let edit = |i: u32| format!("{base}\n// keystroke {i}\n");

    // Warm-up: prime Salsa's caches.
    for i in 0..WARMUP {
        let notif = did_change(&uri, i64::from(i) + 2, &edit(i));
        let _ = round_trip(&mut state, notif);
    }

    // Measured: steady-state per-didChange cost. The notification is built
    // outside the clock; everything the server does with it is inside.
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    for i in 0..ITERS {
        let notif = did_change(&uri, i64::from(WARMUP + i) + 2, &edit(WARMUP + i));
        let start = Instant::now();
        let published = round_trip(&mut state, notif);
        let elapsed = start.elapsed();
        std::hint::black_box(published);
        worst = worst.max(elapsed);
        total += elapsed;
    }

    // Microseconds as well as whole milliseconds: at ~6 ms the integer figure
    // has one significant digit, which is not enough to see a handler step
    // being added or removed — and «does this number move when the handler
    // does» is the property this file exists to have.
    let avg = total / ITERS;
    eprintln!(
        "didChange round-trip over canonical_200.fossil: avg={}ms ({}µs) worst={}ms ({}µs) \
         (design goal <100ms; hard-gate budget <{BUDGET_MS}ms)",
        avg.as_millis(),
        avg.as_micros(),
        worst.as_millis(),
        worst.as_micros(),
    );

    let worst_ms = worst.as_millis();
    assert!(
        worst_ms < BUDGET_MS,
        "didChange round-trip blew the margined budget: worst={worst_ms}ms >= {BUDGET_MS}ms \
         (avg={}ms). This margin tolerates CI noise but catches algorithmic blow-up — \
         a real regression here means the analysis pipeline got asymptotically slower.",
        avg.as_millis(),
    );
}
