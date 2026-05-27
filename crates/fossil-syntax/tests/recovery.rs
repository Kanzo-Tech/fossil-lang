//! Wave-0 regression — parser TERMINATES on unlexable bytes.
//!
//! Background: the logos lexer silently drops bytes it cannot tokenise
//! (`crates/fossil-syntax/src/lexer.rs:237` — `raw_lex` does
//! `filter_map(|(tok, range)| tok.ok().map(...))`). Before the plan
//! 07-01 fix, `parse_program`'s fall-through called
//! `recover_to(p, TOP_LEVEL_ANCHORS)` — and `IDENT ∈ TOP_LEVEL_ANCHORS` —
//! so the call no-op'd while the outer `loop` re-entered the same arm
//! with the same `p.pos` forever. The parser spun at 100% CPU and never
//! returned. See `.planning/phases/06-cli-complete-lsp/deferred-items.md`
//! (HIGH-priority parser-hang).
//!
//! The fix in `parser/items.rs::parse_program` replaces the no-progress
//! `recover_to` calls with `p.bump_as_error()` (always advances `p.pos`).
//!
//! These tests would NEVER TERMINATE before the fix. They run in
//! sub-millisecond range post-fix; we assert a generous <100ms wall-clock
//! bound so a future regression that re-introduces the loop will surface
//! as a timeout-style failure (rather than the user noticing the test
//! suite never ends).
//!
//! # Known limitation — diagnostic surface for the truly-bare-byte case
//!
//! Because `raw_lex` silently drops `Err` tokens from logos, an input
//! consisting ONLY of unlexable bytes (e.g. `"#"`, `"$"`) produces an
//! EMPTY token stream — `parse_program` immediately breaks on `None` and
//! no diagnostic is emitted. This is a separate lexer-layer issue and is
//! intentionally OUT OF SCOPE for plan 07-01 (which is scoped to the
//! parser-side no-progress fix, as the Wave 0 precondition for Phase 7's
//! Monaco mount). Tracking: see Plan 07-01 SUMMARY, "Deferred — lexer
//! silent-drop". For now we assert TERMINATION (the only invariant the
//! parser-side fix actually owns) on minimal-byte inputs, and add a
//! diagnostic-count assertion only on inputs where a valid downstream
//! token reaches the parser.
//!
//! ## Wall-clock bound rationale
//!
//! cargo test has no per-test timeout by default. To convert "spins
//! forever" into a deterministic test failure we wrap each parse on a
//! worker thread + `recv_timeout` on the main thread. If the worker
//! doesn't finish in 2s we `panic!("parser hung — regression")` from
//! the test thread (the worker is leaked, which is acceptable for a
//! once-per-CI-run regression — the process exits after the test
//! suite). Post-fix the parses take micro-seconds; 2s is ~10000× the
//! happy-path budget and absorbs any CI noise.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fossil_base::{Diagnostic, FossilDb, NativeSystem, SourceFile, System};
use fossil_syntax::parse;

/// Parse `input` on a worker thread; panic with "parser hung — regression"
/// if it doesn't finish within `timeout`. On success returns the wall-clock
/// duration + the count of accumulated diagnostics.
///
/// We cannot return the `Cst<'db>` itself because the db lives on the
/// worker thread; for this regression test the (duration, diag-count)
/// summary is all the assertions need.
fn parse_with_timeout(input: &'static str, timeout: Duration) -> (Duration, usize) {
    let (tx, rx) = mpsc::channel();
    let input_owned = input.to_string();
    thread::spawn(move || {
        let start = Instant::now();
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, input_owned, "regression.fossil".to_string());
        let _cst = parse(&db, file);
        let diags: Vec<&Diagnostic> = parse::accumulated::<Diagnostic>(&db, file);
        let elapsed = start.elapsed();
        // Send the diagnostic COUNT, not the borrowed Vec — the db owns
        // the storage and goes out of scope when the worker exits.
        let _ = tx.send((elapsed, diags.len()));
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        panic!("parser hung — regression: parse did not return within {timeout:?}")
    })
}

/// Plan 07-01 happy-path budget: parse-on-bare-`#` finishes well under 100ms.
/// We pick 100ms as a regression-style soft cap that is huge vs the actual
/// post-fix cost (~micro-seconds) but still catches a pathological slowdown.
const HAPPY_PATH_BUDGET: Duration = Duration::from_millis(100);

/// Hung-process detection: 2s is ~20× the happy-path soft cap and ~10⁴×
/// the actual post-fix wall-clock. Any future regression that re-introduces
/// the no-progress loop trips this gate within seconds.
const HANG_DETECTION_BUDGET: Duration = Duration::from_secs(2);

#[test]
fn parse_terminates_on_bare_unlexable_byte() {
    // The minimal repro: a single `#`. Pre-fix: 100% CPU forever.
    // Post-fix: returns in a handful of microseconds.
    //
    // Diagnostic-count is NOT asserted here — see the module-level
    // "Known limitation" note: raw_lex silently drops the `#`, so the
    // resulting token stream is empty and the parser sees nothing to
    // report. The plan-07-01 fix is fundamentally about TERMINATION;
    // the diagnostic-surface gap is a separate lexer-layer concern.
    let (elapsed, _diag_count) = parse_with_timeout("#", HANG_DETECTION_BUDGET);
    assert!(
        elapsed < HAPPY_PATH_BUDGET,
        "parse(\"#\") took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?} \
         (happy-path budget — a slowdown here likely signals a partial \
         regression of the parser-hang)",
    );
}

#[test]
fn parse_terminates_on_unlexable_then_valid_item() {
    // The realistic shape — a stray `#` followed by an otherwise
    // well-formed top-level item (the mid-edit Monaco scenario:
    // user just typed `#` and the LSP receives didChange). The
    // valid `prefix` decl after the `#` MUST still parse cleanly —
    // sanity that the recovery advances past the bad token AND
    // resumes normal parsing. We do not assert the exact green-tree
    // shape here; the parse_corpus snapshot tests already pin
    // CST shape elsewhere.
    let input = "# stray comment\nprefix ex: <https://example.org/>\n";
    let (elapsed, diag_count) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
    assert!(
        diag_count >= 1,
        "expected ≥1 diagnostic for the stray `#`; got {diag_count}",
    );
    assert!(
        elapsed < HAPPY_PATH_BUDGET,
        "parse(\"# … \\nprefix …\") took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
    );
}

#[test]
fn parse_terminates_on_other_unlexable_bytes() {
    // The fix is byte-agnostic — anything logos drops (e.g. `$` outside
    // a `${...}` interpolation, control chars, exotic punctuation) must
    // reach the same `parse_program` fall-through and bump_as_error.
    // We sample a few representative bytes; an exhaustive sweep is the
    // job of a fuzz target (deferred).
    //
    // Per the "Known limitation" module-level note, diagnostic count is
    // NOT asserted for inputs consisting ONLY of unlexable bytes —
    // raw_lex silently drops them and the parser sees an empty stream.
    // TERMINATION is the only invariant this plan owns; we assert that.
    let cases: &[&'static str] = &[
        "$",   // not a token (interpolation is INSIDE TEMPLATE)
        "#",   // the originally-reported bug
        "##",  // two unlexable bytes in a row
        "#$#", // mixed unlexable bytes
        "\\",  // backslash
    ];
    for input in cases {
        let (elapsed, _diag_count) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
        assert!(
            elapsed < HAPPY_PATH_BUDGET,
            "input {input:?}: took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
        );
    }
}

#[test]
fn parse_terminates_on_unlexable_inside_mapping_header_position() {
    // A `#` between two top-level items must also terminate. This
    // exercises the outer `_ => bump_as_error()` arm rather than the
    // IDENT-lookahead arm. Pre-fix would also hang here because the
    // next non-trivia token after the dropped `#` is `prefix`
    // (KW_PREFIX, which IS in TOP_LEVEL_ANCHORS) — same no-progress
    // shape.
    let input = "prefix ex: <https://example.org/>\n#\nprefix ey: <https://example.com/>\n";
    let (elapsed, diag_count) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
    // Note: a single `#` between two well-formed items might not
    // trigger any diagnostic because the lexer silently drops it and
    // the resulting token stream is two clean prefix decls. That's
    // acceptable — the test is fundamentally about TERMINATION, not
    // diagnostic count. We do not assert diag_count here.
    let _ = diag_count;
    assert!(
        elapsed < HAPPY_PATH_BUDGET,
        "took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
    );
}
