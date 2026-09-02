//! The ADVISORY Criterion benchmark for the `didChange` round-trip budget.
//!
//! It calls [`fossil_lsp::handle_notification`] with a real
//! `textDocument/didChange` notification over `canonical_200.fossil` —
//! the same function `tests/didchange_budget.rs` gates and the same function
//! `src/main.rs` calls off the wire. There is no `round_trip` here any more, so
//! there is nothing left to keep in sync.
//!
//! # There were two copies, and the docblock claimed they agreed
//!
//! This file carried its own `round_trip`, a hand-rolled `def_map` +
//! `typecheck_mapping` loop, under a line reading «identical to the budget
//! test's, kept in sync». It was not: the budget test had collapsed onto
//! `fossil_mir::program_diagnostics`, which drains `lower_to_mir_pg`, so every
//! lowering a keystroke pays for was outside this clock. That was repaired in
//! `eccb7db` by editing the copy — the third time in these two crates a comment
//! asserted an agreement nothing checked.
//!
//! Both copies existed because `LspState` and the handlers lived in a binary
//! target and each took an `&Connection`. Neither is true now, and «call the
//! same function» is the only mechanism that was ever going to hold.
//!
//! # Status: ADVISORY
//!
//! Criterion's committed baseline + 20%-regression detection is reliable only
//! on a pinned / self-hosted runner (stable CPU, no neighbour noise). On shared
//! CI it is INFORMATIONAL — run it, record the numbers, but do not BLOCK on the
//! regression check (that would flake). The blocking gate is the margined
//! wall-clock budget test. The design GOAL this benchmark tracks is `< 100ms`
//! on dev hardware; the benchmark reports the true steady-state cost so a real
//! perf regression is visible on a controlled runner.
//!
//! Run: `cargo bench -p fossil-lsp`. The first run writes the baseline under
//! `target/criterion/`; subsequent runs compare against it.

use std::path::PathBuf;
use std::str::FromStr as _;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use fossil_lsp::LspState;
use lsp_server::Notification;
use lsp_types::Uri;
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as NotificationTrait,
};
use serde_json::json;

/// The path the program is opened under — the REAL one, because the shape
/// document is resolved relative to the program. See the hard gate's module
/// docs: a synthetic path resolves no contract, and the benchmark then measures
/// a program that checks nothing.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/canonical_200.fossil")
        .canonicalize()
        .expect("canonicalise canonical_200.fossil")
}

fn fixture_uri() -> Uri {
    Uri::from_str(&format!("file://{}", fixture_path().display())).expect("a file:// URI")
}

fn fixture() -> String {
    std::fs::read_to_string(fixture_path()).expect("read canonical_200.fossil")
}

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

fn did_change(uri: &Uri, version: i64, text: &str) -> Notification {
    Notification {
        method: DidChangeTextDocument::METHOD.to_string(),
        params: json!({
            "textDocument": { "uri": uri.as_str(), "version": version },
            "contentChanges": [ { "text": text } ]
        }),
    }
}

fn bench_didchange(c: &mut Criterion) {
    let base = fixture();
    let uri = fixture_uri();

    let mut group = c.benchmark_group("lsp_didchange");
    // A handful of samples is enough for a fixture this size; keep wall time low.
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("canonical_200_round_trip", |b| {
        // Build a warm server once; each iteration is a distinct edit so
        // `set_text` genuinely bumps the revision (the realistic editor case).
        // `didOpen` is the handler too — it registers the shape document the
        // program names, without which this would measure the error path.
        let mut state = LspState::new();
        let _ = fossil_lsp::handle_notification(&mut state, did_open(&uri, &base))
            .expect("didOpen params decode");
        let _ = fossil_lsp::handle_notification(
            &mut state,
            did_change(&uri, 2, &format!("{base}\n// warm\n")),
        )
        .expect("didChange params decode");
        let mut i = 2i64;
        b.iter_batched(
            || {
                i += 1;
                // VALID-syntax edit (a `//` comment line — never a bare `#`).
                // Built OUTSIDE the timed section: serialising the buffer into
                // the params is the client's cost, not the server's.
                did_change(&uri, i, &format!("{base}\n// edit {i}\n"))
            },
            |notif| {
                std::hint::black_box(
                    fossil_lsp::handle_notification(&mut state, notif)
                        .expect("didChange params decode"),
                )
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_didchange);
criterion_main!(benches);
