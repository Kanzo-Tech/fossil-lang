//! The gate on the SIZE of the revalidation walk a keystroke performs.
//!
//! `didchange_budget.rs` gates the clock and
//! `fossil-hir/tests/invalidation_regression.rs` gates the count of queries
//! that RE-EXECUTE. Neither can see this, and the gap is not small.
//!
//! Salsa's revalidation is a depth-first walk of the reachable memo graph that
//! mostly *ends* in `DidValidateMemoizedValue` — the memo was still good.
//! `WillExecute` is emitted only where the walk gives up and runs a body, so a
//! walk that grew tenfold while still validating everything it touched emits
//! exactly as many `WillExecute` events as before. `MAX_REEXECUTIONS = 18` is
//! blind to it by construction, and on a 200-line file so is a millisecond
//! budget with a 400 ms ceiling over a 3 ms measurement.
//!
//! So this counts the other event. `DidValidateMemoizedValue` is emitted from
//! salsa's `Memo::mark_as_verified`, which has exactly two callers in
//! `function/maybe_changed_after.rs`: the durability shortcut
//! (`update_shallow`) and the tail of a deep verify. A memo already verified in
//! the current revision returns `ShallowUpdate::Verified` *before* either, so
//! nothing is counted twice. The number is therefore **distinct memos the walk
//! touched in this revision**.
//!
//! # The durability shortcut cannot fire in this compiler, and it does not matter
//!
//! Nothing in `crates/` sets a `salsa::Durability` — `grep -rn Durability
//! crates/` finds only prose — so every input field is `Durability::LOW`, the
//! `Default`. Salsa's shortcut asks `last_changed_revision(memo.durability) <=
//! verified_at`, and `last_changed_revision(LOW)` is `revisions[0]`, which IS
//! the current revision and advances on every `set_text`. The comparison is
//! `current <= previous`. **It is false every time.** The shortcut is dead code
//! here, and every keystroke walks the whole reachable graph.
//!
//! That reads like a finding and it is not one, which is the reason this
//! paragraph is in the tree rather than in a commit message nobody will find
//! again. `Registry::rows`, `FileRegistry::entries` and `SourceFile::path` were
//! all raised to `Durability::HIGH` and this test re-run: **60 revalidations
//! before, 60 after, and the per-query breakdown identical to the event.** A
//! derived memo's durability is the MINIMUM over its inputs, and every memo in
//! the walk below reads the edited buffer's `SourceFile::text`, which is LOW
//! and has to stay LOW — it is the field the user is typing into. Raising three
//! other fields cannot raise a minimum that `text` already floors. The change
//! was reverted; what stayed is this file, because the count is the thing worth
//! having and nobody had it.
//!
//! # What the walk is, measured
//!
//! Over `tests/fixtures/canonical_200.fossil` (15 mappings), one steady-state
//! keystroke: **60 memos revalidated, 22 executed**. The 60 are
//! `body`, `spans`, `typecheck_mapping` and `lower_to_mir_pg` for the mappings
//! the edit did not touch, plus three file-keyed ones (`check_identities`,
//! `subject_templates`, `decode_shape_document`). That is four per mapping and
//! a constant — the per-mapping invalidation barrier doing exactly what
//! `invalidation_regression.rs` claims for it, seen from the other side. There
//! is no waste in it to remove.
//!
//! # Why the bound is derived and not a constant
//!
//! A hard `60` goes red the day someone adds a mapping to the fixture, for no
//! reason. The property worth gating is that the walk stays LINEAR in the
//! mappings, so the ceiling is computed from the mapping count the db reports.
//! A new file-keyed query costs one and fits in the slack; a new *per-mapping*
//! query, or anything quadratic, does not.
//!
//! # The red run, and the reason this file exists at all
//!
//! A guard nobody has watched go red is a guard nobody has tested, so it was
//! made to. A fifth per-mapping `#[salsa::tracked]` query reading `spans` was
//! added to `fossil_ide::diagnostics` — a real query on the real keystroke
//! path, not a lowered constant — and this test run again:
//!
//! ```text
//! before:  revalidated worst=60 (ceiling 65), executed worst=22   PASS
//! after:   revalidated worst=75 (ceiling 65), executed worst=22   FAIL
//! ```
//!
//! **`executed` did not move.** Fifteen memos joined the walk a keystroke pays
//! for and the `WillExecute` count — the thing `MAX_REEXECUTIONS = 18` gates —
//! was identical before and after, because the new query validated for every
//! mapping instead of running. That is the blind spot stated as a measurement,
//! and it is the whole argument for counting the other event. The probe was
//! reverted; the numbers are what stayed.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fossil_lsp::LspState;
use lsp_server::Notification;
use lsp_types::Uri;
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as NotificationTrait,
};
use serde_json::json;

/// Revalidations allowed per mapping. Measured: four — `body`, `spans`,
/// `typecheck_mapping`, `lower_to_mir_pg`, one each for every mapping the edit
/// did not touch. A FIFTH per-mapping tracked query blows the ceiling on the
/// canonical fixture, which is the point: register it here with its reason, the
/// way `invalidation_regression.rs` requires of the re-execution keyset.
const PER_MAPPING: usize = 4;

/// Slack for the file-keyed part of the walk. Measured: three
/// (`check_identities`, `subject_templates`, `decode_shape_document`), so this
/// leaves room for two more. It is small ON PURPOSE and the size was chosen by
/// experiment, not taste: a fifth per-mapping query measured 75 against this
/// ceiling of 65. A slack of 15 — the first number written here — would have
/// absorbed it and the guard would have gated nothing. See the red run in the
/// module docs.
const FILE_LEVEL_SLACK: usize = 5;

/// Warm-up keystrokes before the counter is read. The first `didChange` after
/// `didOpen` walks a graph that has never been revalidated, which is not the
/// steady state an editor is in.
const WARMUP: u32 = 3;

/// Keystrokes measured. Each is counted alone and the WORST asserted, because a
/// walk that grows need not grow on every keystroke.
const ITERS: u32 = 5;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/canonical_200.fossil")
        .canonicalize()
        .expect("canonicalise canonical_200.fossil")
}

fn fixture_uri() -> Uri {
    Uri::from_str(&format!("file://{}", fixture_path().display())).expect("a file:// URI")
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

/// The two counters, installed on the real server.
struct Counters {
    validated: Arc<AtomicUsize>,
    executed: Arc<AtomicUsize>,
}

impl Counters {
    fn install() -> (LspState, Self) {
        let validated = Arc::new(AtomicUsize::new(0));
        let executed = Arc::new(AtomicUsize::new(0));
        let (v, e) = (validated.clone(), executed.clone());
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> =
            Box::new(move |event| match event.kind {
                salsa::EventKind::DidValidateMemoizedValue { .. } => {
                    v.fetch_add(1, Ordering::Relaxed);
                }
                salsa::EventKind::WillExecute { .. } => {
                    e.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            });
        (
            LspState::with_event_callback(callback),
            Self {
                validated,
                executed,
            },
        )
    }

    fn reset(&self) {
        self.validated.store(0, Ordering::Relaxed);
        self.executed.store(0, Ordering::Relaxed);
    }

    fn read(&self) -> (usize, usize) {
        (
            self.validated.load(Ordering::Relaxed),
            self.executed.load(Ordering::Relaxed),
        )
    }
}

#[test]
fn the_revalidation_walk_stays_linear_in_the_mappings() {
    let base = std::fs::read_to_string(fixture_path()).expect("read canonical_200.fossil");
    let uri = fixture_uri();

    let (mut state, counters) = Counters::install();

    let opened = fossil_lsp::handle_notification(&mut state, did_open(&uri, &base))
        .expect("didOpen params decode");
    assert_eq!(opened.len(), 1, "didOpen publishes one notification");

    // The ceiling is computed from what the fixture actually contains, so a
    // fixture edit moves the bound with it and only a change in the SHAPE of the
    // walk goes red.
    let file = state.get(&uri).expect("the buffer is open after didOpen");
    let mappings = fossil_hir::def_map::def_map(state.db(), file)
        .mappings(state.db())
        .len();
    assert!(mappings > 0, "the fixture must contain mappings");
    let ceiling = PER_MAPPING * mappings + FILE_LEVEL_SLACK;

    let edit = |i: u32| format!("{base}\n// keystroke {i}\n");

    for i in 0..WARMUP {
        let notif = did_change(&uri, i64::from(i) + 2, &edit(i));
        let _ =
            fossil_lsp::handle_notification(&mut state, notif).expect("didChange params decode");
    }

    let mut worst_validated = 0usize;
    let mut worst_executed = 0usize;
    for i in 0..ITERS {
        let notif = did_change(&uri, i64::from(WARMUP + i) + 2, &edit(WARMUP + i));
        counters.reset();
        let published =
            fossil_lsp::handle_notification(&mut state, notif).expect("didChange params decode");
        std::hint::black_box(published);
        let (validated, executed) = counters.read();
        worst_validated = worst_validated.max(validated);
        worst_executed = worst_executed.max(executed);
    }

    eprintln!(
        "didChange over canonical_200.fossil ({mappings} mappings): \
         revalidated worst={worst_validated} (ceiling {ceiling}), executed worst={worst_executed}"
    );

    assert!(
        worst_validated > 0,
        "no DidValidateMemoizedValue events at all — either the callback is not \
         wired or salsa stopped emitting the event this file is built on. Either \
         way it is measuring nothing, and a green tick here would be a lie."
    );
    assert!(
        worst_validated <= ceiling,
        "the revalidation walk grew: {worst_validated} memos revalidated on one \
         keystroke over {mappings} mappings, ceiling {ceiling} \
         ({PER_MAPPING}/mapping + {FILE_LEVEL_SLACK}). Salsa re-walks everything \
         whose durability says it might have changed, and every memo downstream \
         of the edited buffer is `Durability::LOW`, so the walk is the whole \
         reachable graph and its size is the per-mapping fan-out. A FIFTH \
         per-mapping tracked query is the usual cause; anything quadratic (a \
         query that reads all mappings, keyed by one) is the other. Neither \
         `MAX_REEXECUTIONS` nor the millisecond budget can see this, which is \
         why the bound lives here. Raise `PER_MAPPING` only with the new query \
         named and its cost written down."
    );
}
