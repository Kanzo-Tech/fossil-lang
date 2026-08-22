//! Unlexable bytes — the parser TERMINATES, and it REPORTS.
//!
//! Two bugs met on the same input, and both are fixed:
//!
//! 1. **The hang.** `parse_program`'s fall-through called
//!    `recover_to(p, TOP_LEVEL_ANCHORS)` — and `IDENT ∈ TOP_LEVEL_ANCHORS` —
//!    so the call no-op'd while the outer `loop` re-entered the same arm
//!    with the same `p.pos` forever. The parser spun at 100% CPU and never
//!    returned. `parser/items.rs::parse_program` uses `p.bump_as_error()`
//!    instead, which always advances `p.pos`.
//!
//! 2. **The silence.** `raw_lex` dropped logos's `Err` variants, so an input
//!    of ONLY unlexable bytes — `"#"`, `"$$"` — lexed to an EMPTY token
//!    stream: `parse_program` broke on `None` immediately, the CST came back
//!    empty, and the LSP's Problems panel showed nothing for a file the user
//!    could see was wrong. `lexer::raw_lex_lossless` keeps the range and
//!    `indent::lex_with_indents` emits it as a `SyntaxKind::ERROR` token
//!    carrying the byte, which `bump_as_error` reports as
//!    `unexpected character `#` — no token starts with it`.
//!
//! These tests would NEVER TERMINATE before the first fix, and asserted
//! nothing about diagnostics before the second. They run in sub-millisecond
//! range; we assert a generous <100ms wall-clock bound so a future regression
//! that re-introduces the loop will surface as a timeout-style failure
//! (rather than the user noticing the test suite never ends).
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

use fossil_base::test_support::NativeSystem;
use fossil_base::{Diagnostic, FossilDb, SourceFile, System};
use fossil_syntax::parse;

/// Parse `input` on a worker thread; panic with "parser hung — regression"
/// if it doesn't finish within `timeout`. On success returns the wall-clock
/// duration + the accumulated diagnostic messages.
///
/// We cannot return the `Cst<'db>` itself, nor the `&Diagnostic`s, because the
/// db lives on the worker thread and drops when it exits — so the messages are
/// copied out as owned `String`s.
fn parse_with_timeout(input: &'static str, timeout: Duration) -> (Duration, Vec<String>) {
    let (tx, rx) = mpsc::channel();
    let input_owned = input.to_string();
    thread::spawn(move || {
        let start = Instant::now();
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, input_owned, "regression.fossil".to_string());
        let _cst = parse(&db, file);
        let diags: Vec<&Diagnostic> = parse::accumulated::<Diagnostic>(&db, file);
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        let elapsed = start.elapsed();
        let _ = tx.send((elapsed, messages));
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        panic!("parser hung — regression: parse did not return within {timeout:?}")
    })
}

/// Happy-path budget: parse-on-bare-`#` finishes well under 100ms.
/// We pick 100ms as a regression-style soft cap that is huge vs the actual
/// post-fix cost (~micro-seconds) but still catches a pathological slowdown.
const HAPPY_PATH_BUDGET: Duration = Duration::from_millis(100);

/// Hung-process detection: 2s is ~20× the happy-path soft cap and ~10⁴×
/// the actual post-fix wall-clock. Any future regression that re-introduces
/// the no-progress loop trips this gate within seconds.
const HANG_DETECTION_BUDGET: Duration = Duration::from_secs(2);

#[test]
fn parse_reports_the_bare_unlexable_byte() {
    // The minimal repro: a single `#`. Once 100% CPU forever, then a clean
    // return with nothing to show for it. Now exactly one diagnostic, and it
    // says which character — the byte is the only content the file has, so a
    // generic "unexpected token" would name nothing at all.
    let (elapsed, diags) = parse_with_timeout("#", HANG_DETECTION_BUDGET);
    assert_eq!(
        diags,
        vec!["unexpected character `#` — no token starts with it"],
        "parse(\"#\") must produce exactly one diagnostic, naming the byte",
    );
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
    // valid binding after the `#` MUST still parse cleanly — sanity
    // that the recovery advances past the bad token AND resumes normal
    // parsing. We do not assert the exact green-tree shape here; the
    // parse_corpus snapshot tests already pin CST shape elsewhere.
    // `stray` and `comment` are two bare IDENTs that start no production, so
    // they report too — one diagnostic each, and that is the fall-through arm
    // doing its job rather than a regression. What this test is about is the
    // `#`: that it is NAMED, and that the parse returns.
    let input = "# stray comment\nusers := io.csv(\"users.csv\")\n";
    let (elapsed, diags) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
    assert!(
        diags.iter().any(|m| m.contains('#')),
        "expected a diagnostic naming the stray `#`; got {diags:?}",
    );
    assert!(
        elapsed < HAPPY_PATH_BUDGET,
        "parse(\"# … \\nusers := …\") took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
    );
}

#[test]
fn parse_reports_every_unlexable_byte() {
    // Byte-agnostic — anything logos rejects (`$` outside a hole, control
    // chars, exotic punctuation) reaches the same `parse_program`
    // fall-through and the same `bump_as_error`. We sample a few
    // representative bytes; an exhaustive sweep is the job of a fuzz target
    // (deferred).
    //
    // One diagnostic PER BYTE, not one per run: logos rejects a byte at a
    // time, each becomes its own ERROR token, and each is named. `#$#` is
    // three characters and three messages.
    let cases: &[(&'static str, usize)] = &[
        ("$", 1),   // not a token (a hole's `{` is only a hole inside a string)
        ("#", 1),   // the originally-reported bug
        ("##", 2),  // two unlexable bytes in a row
        ("#$#", 3), // mixed unlexable bytes
        ("\\", 1),  // backslash
    ];
    for (input, want) in cases {
        let (elapsed, diags) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
        assert_eq!(
            diags.len(),
            *want,
            "input {input:?}: expected {want} diagnostic(s), got {diags:?}",
        );
        assert!(
            diags.iter().all(|m| m.starts_with("unexpected character")),
            "input {input:?}: every diagnostic must name its character; got {diags:?}",
        );
        assert!(
            elapsed < HAPPY_PATH_BUDGET,
            "input {input:?}: took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
        );
    }
}

#[test]
fn parse_reports_an_unlexable_byte_between_two_good_items() {
    // A `#` on its own line between two well-formed items. This exercises the
    // outer `_ => bump_as_error()` arm rather than the IDENT-lookahead arm.
    //
    // It is the case the old test explicitly gave up on: the byte was dropped,
    // the stream was two clean items, and the file looked perfect to every tool
    // while the user was staring at the `#`. Exactly one diagnostic, and the two
    // items around it still parse.
    //
    // The two items used to be `prefix` declarations, which is what this file
    // reached for whenever it wanted something short and certainly valid. They
    // are source bindings now — a `prefix` line is itself a diagnostic, and
    // asserting «only the `#` is reported» around two of them would have been
    // asserting the opposite of what the test says.
    let input = "a := io.csv(\"a.csv\")\n#\nb := io.csv(\"b.csv\")\n";
    let (elapsed, diags) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
    assert_eq!(
        diags,
        vec!["unexpected character `#` — no token starts with it"],
        "the byte between two good items must be reported, and only it",
    );
    assert!(
        elapsed < HAPPY_PATH_BUDGET,
        "took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
    );
}
