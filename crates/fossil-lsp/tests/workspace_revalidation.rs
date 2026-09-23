//! The two things `didchange_revalidation.rs` says it does not cover: a
//! workspace with several buffers open, and `didOpen`.
//!
//! That file measures one buffer and one `didChange`, and the commit that landed
//! it (`ea58ee5`) named the gap in its message: «It says nothing about a
//! workspace larger than one open program, nothing about `didOpen`.» A message
//! is not somewhere anybody looks, so the answer lands here instead — and it is
//! a NEGATIVE: the walk does not grow with the workspace, and an open walks the
//! same graph a keystroke does. It is written down with its numbers because a
//! negative nobody recorded is a negative somebody measures again.
//!
//! # What a notification costs, measured
//!
//! `canonical_200.fossil` (15 mappings) and `workspace_peer.fossil` (4), each
//! in a workspace of 2, 3, 6 and 14 open buffers:
//!
//! ```text
//!                           15 mappings          4 mappings
//!   steady didChange     60 valid + 22 exec   16 valid + 11 exec
//!     walk                      82                   27
//!   didOpen              1 valid + 83 exec    1 valid + 28 exec
//!     walk                      84                   29
//! ```
//!
//! **Every one of those numbers is identical at 2 buffers and at 14**, and the
//! `didOpen` row is identical whether the buffer is opened into 1 other buffer
//! or 13. Two mapping counts is what makes them a line rather than a point:
//!
//! ```text
//!   validated = 4m      4(m-1) for the mappings the edit did not touch, plus
//!                       `spans` for the one it did and the three file-keyed
//!                       memos (check_identities, subject_templates,
//!                       decode_shape_document)
//!   executed  = m + 7   mapping_cst_node for every mapping, plus parse,
//!                       def_map, line_offsets, a DatabaseKeyIndex, and
//!                       body / typecheck_mapping / lower_to_mir_pg for the one
//!                       mapping the edit touched
//!   WALK      = 5m + 7  five per mapping — mapping_cst_node, body, spans,
//!                       typecheck_mapping, lower_to_mir_pg — and seven that
//!                       are keyed by the file
//! ```
//!
//! Checked at both counts: 60 = 4·14 + 1 + 3 and 22 = 15 + 4 + 3 at fifteen
//! mappings, 16 = 4·3 + 1 + 3 and 11 = 4 + 4 + 3 at four.
//!
//! A `didOpen` is the same graph with nothing memoised yet: `5m + 8` executed
//! and one validated, so `5m + 9`. The two over a keystroke are an unnamed
//! `DatabaseKeyIndex` ingredient — twice on an open, once on a keystroke — and
//! the one memo that validates. Which they are did not turn out to matter; that
//! they are the same two at four mappings and at fifteen is what the bound
//! needs.
//!
//! There is no workspace term. A keystroke's walk is the reachability of the
//! memo graph from the edited buffer, and no other buffer is in it: the
//! `didChange` path publishes for one URI, and the cross-file surface
//! (`LspState::open_files`, which goto-def and completion resolve against) is
//! on the REQUEST path, not the notification path.
//!
//! # Why this gates the WALK and not the revalidations
//!
//! `didchange_revalidation.rs` bounds `DidValidateMemoizedValue` alone, and the
//! measurement that says that half is not a stable quantity is here. The first
//! keystroke after ANY `didOpen` re-executes what it would otherwise validate:
//! `FileRegistry::entries` is one input for the whole path→file map by design —
//! `fossil-base/src/files.rs` says so, «Registering any file invalidates every
//! `file_at` reader» — so opening a buffer un-verifies the readers in all the
//! others. On `canonical_200.fossil`, the keystroke immediately after an open:
//!
//! ```text
//!    2 buffers open:   46 validated + 36 executed = 82
//!   13 buffers open:    7 validated + 75 executed = 82
//!   steady state:      60 validated + 22 executed = 82
//! ```
//!
//! **The walk is 82 in all three and the `validated` half is 60, 46 or 7.** A
//! bound on `validated` alone gets GREENER as work moves from validation to
//! execution, which is the wrong direction; the sum is the quantity that does
//! not move. By the second keystroke it is back to 60/22, so this is a
//! transient and not a defect — but it is exactly why the number gated here is
//! the sum.
//!
//! (What drives the split is salsa tracked-struct id churn: after an open the
//! mapping ids come back as `Id(140cg1)` — a reused id at generation 1 — and a
//! mapping whose id moved cannot validate. That is salsa's business, the walk
//! is the same size either way, and nothing here is worth changing for it.)
//!
//! # What went red
//!
//! Two probes, both written, both measured, both reverted.
//!
//! **Publishing diagnostics for every open buffer on `didChange`** — six lines
//! in `fossil_lsp::handle_notification`, and the change this file exists to
//! catch. The keystroke walk over `canonical_200.fossil`:
//!
//! ```text
//!   buffers open      2     3     6    14
//!   before           82    82    82    82
//!   after           109   135   213   421
//! ```
//!
//! and `didchange_revalidation.rs` **stayed green at 60 revalidated, 22
//! executed, through all of it**, because it opens one buffer and there is
//! nothing to republish. That is the workspace blind spot as a measurement, and
//! the whole argument for this file.
//!
//! **A fifth per-mapping tracked query** in `fossil_ide::diagnostics` — the same
//! probe `didchange_revalidation.rs` used, re-run here. `5m + 7` became `6m + 7`:
//!
//! ```text
//!                    canonical (15)      peer (4)
//!   keystroke        82 -> 97  (85)    27 -> 31  (30)
//!   didOpen          84 -> 99  (87)    29 -> 33  (32)
//! ```
//!
//! Ceilings in brackets; all four red. **The peer is what makes [`SLACK`] mean
//! something**: four mappings buy a new per-mapping query only four memos, so a
//! slack of three catches it by one and a slack of five would not catch it at
//! all. On the same probe `didchange_revalidation.rs` reported 75 revalidated
//! against its ceiling of 65 — the number its own docs record, reproduced.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
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

/// Memos the walk touches per mapping. Measured: five — `mapping_cst_node`,
/// `body`, `spans`, `typecheck_mapping`, `lower_to_mir_pg`. For the mappings the
/// edit did not touch, `mapping_cst_node` re-executes and the other four
/// validate; for the one it did, three more re-execute. That split is what
/// `didchange_revalidation.rs` bounds. This file does not care which is which —
/// see its module docs for why.
const PER_MAPPING: usize = 5;

/// The file-keyed part of a keystroke's walk. Measured: seven — `parse`,
/// `def_map`, `line_offsets`, `check_identities`, `subject_templates`,
/// `decode_shape_document` and one `DatabaseKeyIndex`.
const FILE_LEVEL: usize = 7;

/// A `didOpen` walks two more than a keystroke. Measured 84 against 82 (15
/// mappings) and 29 against 27 (4) — the same two at both counts, which is what
/// makes it a constant. See the module docs for what they are.
const COLD_OPEN_EXTRA: usize = 2;

/// Room for what a keystroke may grow by without a story. THREE, and the size
/// is the whole design of this guard: [`PEER`] has four mappings, so a fifth
/// per-mapping query costs it four memos and goes red by one. A slack that
/// absorbed a per-mapping query would gate nothing — the lesson
/// `didchange_revalidation.rs` learned by measuring a slack of 15 that hid a
/// probe worth 15.
const SLACK: usize = 3;

/// Warm-up keystrokes before the counter is read. The first `didChange` after
/// any `didOpen` re-executes rather than validates (see the module docs); the
/// walk is the same size, but the steady state is what a keystroke normally is.
const WARMUP: u32 = 3;

/// Keystrokes measured, worst taken.
const ITERS: u32 = 5;

/// The fifteen-mapping fixture every keystroke number in this repository was
/// measured on.
const CANONICAL: &str = "canonical_200.fossil";

/// The four-mapping peer, which exists so the numbers are a line and not a
/// point. See its own header.
const PEER: &str = "workspace_peer.fossil";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .expect("canonicalise tests/fixtures")
}

fn uri_of(path: &Path) -> Uri {
    Uri::from_str(&format!("file://{}", path.display())).expect("a file:// URI")
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

/// Both salsa memo events, summed. `DidValidateMemoizedValue` is a memo the
/// walk reached and kept; `WillExecute` is one it reached and ran. Their sum is
/// the memos the walk TOUCHED, which is the quantity that does not depend on
/// which of the two happened.
#[derive(Clone, Copy)]
struct Walk {
    validated: usize,
    executed: usize,
}

impl Walk {
    const fn size(self) -> usize {
        self.validated + self.executed
    }
}

impl std::fmt::Display for Walk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} validated + {} executed = {}",
            self.validated,
            self.executed,
            self.size()
        )
    }
}

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

    fn read(&self) -> Walk {
        Walk {
            validated: self.validated.load(Ordering::Relaxed),
            executed: self.executed.load(Ordering::Relaxed),
        }
    }
}

/// What one notification cost in one workspace.
struct Measured {
    mappings: usize,
    cold_open: Walk,
    keystroke: Walk,
}

/// Drive a real server over a workspace holding both fixtures plus `padding`
/// further buffers, and measure `target`'s cold `didOpen` and its steady-state
/// keystroke.
///
/// The workspace is a tempdir rather than `tests/fixtures/` itself, because the
/// padding buffers are files and the repository is not a scratch directory. The
/// two fixtures are copied beside their shape documents, so
/// `register_named_documents` resolves them off disk exactly as it does for a
/// real editor.
fn measure(target: &str, padding: usize) -> Measured {
    let dir = tempfile::tempdir().expect("tempdir");
    for f in [CANONICAL, "canonical_200.shex", PEER, "workspace_peer.shex"] {
        std::fs::copy(fixtures().join(f), dir.path().join(f)).expect("copy a fixture");
    }

    let (mut state, counters) = Counters::install();
    let path = dir.path().join(target);
    let text = std::fs::read_to_string(&path).expect("read the target buffer");
    let uri = uri_of(&path);

    // The workspace the target is opened INTO: the other fixture, then the
    // padding. Opening the target last is the whole point of the second test —
    // an editor opens a file into whatever is already there.
    let other = if target == CANONICAL { PEER } else { CANONICAL };
    let other_path = dir.path().join(other);
    let other_text = std::fs::read_to_string(&other_path).expect("read the peer buffer");
    fossil_lsp::handle_notification(&mut state, did_open(&uri_of(&other_path), &other_text))
        .expect("didOpen");
    for i in 0..padding {
        let p = dir.path().join(format!("pad_{i}.fossil"));
        let padded = format!("{other_text}\n// pad {i}\n");
        std::fs::write(&p, &padded).expect("write a padding buffer");
        fossil_lsp::handle_notification(&mut state, did_open(&uri_of(&p), &padded))
            .expect("didOpen");
    }

    counters.reset();
    let published =
        fossil_lsp::handle_notification(&mut state, did_open(&uri, &text)).expect("didOpen");
    let cold_open = counters.read();
    assert_eq!(published.len(), 1, "didOpen publishes one notification");
    assert_eq!(
        published[0]
            .params
            .get("diagnostics")
            .and_then(|d| d.as_array())
            .map_or(usize::MAX, Vec::len),
        0,
        "{target} must check clean — a walk over a program the checker abandons \
         early is a flattering number"
    );

    let file = state.get(&uri).expect("the buffer is open after didOpen");
    let mappings = fossil_hir::def_map::def_map(state.db(), file)
        .mappings(state.db())
        .len();
    assert!(mappings > 0, "{target} must contain mappings");

    let edit = |i: u32| format!("{text}\n// keystroke {i}\n");
    for i in 0..WARMUP {
        let notif = did_change(&uri, i64::from(i) + 2, &edit(i));
        fossil_lsp::handle_notification(&mut state, notif).expect("didChange");
    }
    let mut keystroke = Walk {
        validated: 0,
        executed: 0,
    };
    for i in 0..ITERS {
        let notif = did_change(&uri, i64::from(WARMUP + i) + 2, &edit(WARMUP + i));
        counters.reset();
        let published = fossil_lsp::handle_notification(&mut state, notif).expect("didChange");
        std::hint::black_box(published);
        let walk = counters.read();
        if walk.size() > keystroke.size() {
            keystroke = walk;
        }
    }

    Measured {
        mappings,
        cold_open,
        keystroke,
    }
}

/// Every buffer count measured. The claim is that the walk is the same at all
/// of them, so the list needs a small one and a large one and nothing in
/// particular between.
const PADDING: [usize; 4] = [0, 1, 4, 12];

#[test]
fn a_keystroke_walks_the_same_graph_however_many_buffers_are_open() {
    for target in [CANONICAL, PEER] {
        let mut sizes = Vec::new();
        for padding in PADDING {
            let m = measure(target, padding);
            let ceiling = PER_MAPPING * m.mappings + FILE_LEVEL + SLACK;
            eprintln!(
                "didChange {target} ({} mappings), {} buffers open: {} (ceiling {ceiling})",
                m.mappings,
                padding + 2,
                m.keystroke
            );
            assert!(
                m.keystroke.size() > 0,
                "no salsa memo events at all — either the callback is not wired \
                 or salsa stopped emitting the events this file is built on, and \
                 a green tick would be a lie"
            );
            assert!(
                m.keystroke.size() <= ceiling,
                "the keystroke walk grew: {} over {} mappings with {} buffers \
                 open, ceiling {ceiling} ({PER_MAPPING}/mapping + {FILE_LEVEL} \
                 + {SLACK} slack). A FIFTH per-mapping tracked query is the \
                 usual cause. Raise {PER_MAPPING} only with the new query named \
                 and its cost written down.",
                m.keystroke,
                m.mappings,
                padding + 2
            );
            sizes.push((padding + 2, m.keystroke.size()));
        }
        let first = sizes[0].1;
        assert!(
            sizes.iter().all(|&(_, size)| size == first),
            "the keystroke walk over {target} depends on how many OTHER buffers \
             are open: {sizes:?}. A keystroke must cost what its own buffer \
             costs. Something on the didChange path is reaching across the \
             workspace — publishing diagnostics for every open buffer, or a \
             query keyed by one file that reads the file registry's contents \
             rather than one entry."
        );
    }
}

#[test]
fn didopen_walks_the_same_graph_a_keystroke_does() {
    for target in [CANONICAL, PEER] {
        let mut sizes = Vec::new();
        for padding in PADDING {
            let m = measure(target, padding);
            let ceiling = PER_MAPPING * m.mappings + FILE_LEVEL + COLD_OPEN_EXTRA + SLACK;
            eprintln!(
                "didOpen   {target} ({} mappings) into {} open buffers: {} (ceiling {ceiling})",
                m.mappings,
                padding + 1,
                m.cold_open
            );
            assert!(
                m.cold_open.size() <= ceiling,
                "the cold didOpen walk grew: {} over {} mappings, ceiling \
                 {ceiling} ({PER_MAPPING}/mapping + {FILE_LEVEL} + \
                 {COLD_OPEN_EXTRA} for the shape document + {SLACK} slack). \
                 Opening a file walks the same memo graph a keystroke in it \
                 walks, with nothing memoised yet; a bigger number means the \
                 open path does something the keystroke path does not.",
                m.cold_open,
                m.mappings
            );
            sizes.push(m.cold_open.size());
        }
        let first = sizes[0];
        assert!(
            sizes.iter().all(|&size| size == first),
            "the didOpen walk over {target} depends on the workspace it is \
             opened into: {sizes:?}, for 1, 2, 5 and 13 buffers already open. \
             Opening a buffer must cost what that buffer costs; nothing else \
             in the workspace names it or is named by it."
        );
    }
}
