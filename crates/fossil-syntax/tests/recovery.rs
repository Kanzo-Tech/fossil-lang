//! Unlexable bytes — the parser TERMINATES, and it REPORTS.
//!
//! Two failures this input invites, and what prevents each:
//!
//! 1. **The hang.** A fall-through in `parse_program` that called
//!    `recover_to(p, TOP_LEVEL_ANCHORS)` — and `IDENT ∈ TOP_LEVEL_ANCHORS` —
//!    would no-op while the outer `loop` re-entered the same arm with the
//!    same `p.pos` forever. `parser/items.rs::parse_program` uses
//!    `p.bump_as_error()`, which always advances `p.pos`.
//!
//! 2. **The silence.** A lex that drops logos's `Err` variants turns an input
//!    of ONLY unlexable bytes — `"#"`, `"$$"` — into an EMPTY token stream:
//!    an empty CST, and nothing shown for a file the user can see is wrong.
//!    `lexer::raw_lex_lossless` keeps the range and
//!    `indent::lex_with_indents` emits it as a `SyntaxKind::ERROR` token
//!    carrying the byte, which `bump_as_error` reports as
//!    `unexpected character `#` — no token starts with it`.
//!
//! They run in sub-millisecond range; we assert a generous <100ms wall-clock
//! bound so a reintroduced loop surfaces as a timeout-style failure (rather
//! than the user noticing the test suite never ends).
//!
//! ## Wall-clock bound rationale
//!
//! cargo test has no per-test timeout by default. To convert "spins
//! forever" into a deterministic test failure we wrap each parse on a
//! worker thread + `recv_timeout` on the main thread. If the worker
//! doesn't finish in 2s we `panic!("parser hung — regression")` from
//! the test thread (the worker is leaked, which is acceptable for a
//! once-per-CI-run regression — the process exits after the test
//! suite). The parses take micro-seconds; 2s is ~10000× the
//! happy-path budget and absorbs any CI noise.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_graph_schema::Problem;
use fossil_syntax::parse;

/// Parse `input` on a worker thread; panic with "parser hung — regression"
/// if it doesn't finish within `timeout`. On success returns the wall-clock
/// duration + the parse diagnostics' problems.
///
/// We cannot return the `Cst<'db>` itself, nor the `&Diagnostic`s, because the
/// db lives on the worker thread and drops when it exits — so the problems are
/// copied out.
fn parse_with_timeout(input: &'static str, timeout: Duration) -> (Duration, Vec<Problem>) {
    let (tx, rx) = mpsc::channel();
    let input_owned = input.to_string();
    thread::spawn(move || {
        let start = Instant::now();
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, input_owned, "regression.fossil".to_string());
        let cst = parse(&db, file);
        let problems: Vec<Problem> = cst
            .diagnostics(&db)
            .iter()
            .map(|d| d.problem.clone())
            .collect();
        let elapsed = start.elapsed();
        let _ = tx.send((elapsed, problems));
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        panic!("parser hung — regression: parse did not return within {timeout:?}")
    })
}

/// Happy-path budget: parse-on-bare-`#` finishes well under 100ms.
/// We pick 100ms as a soft cap that is huge vs the actual cost
/// (~micro-seconds) but still catches a pathological slowdown.
const HAPPY_PATH_BUDGET: Duration = Duration::from_millis(100);

/// Hung-process detection: 2s is ~20× the happy-path soft cap and ~10⁴×
/// the actual wall-clock. A no-progress loop trips this gate within
/// seconds.
const HANG_DETECTION_BUDGET: Duration = Duration::from_secs(2);

#[test]
fn parse_reports_the_bare_unlexable_byte() {
    // The minimal case: a single `#`. Exactly one diagnostic, and it
    // says which character — the byte is the only content the file has, so a
    // generic "unexpected token" would name nothing at all.
    let (elapsed, diags) = parse_with_timeout("#", HANG_DETECTION_BUDGET);
    assert_eq!(
        diags,
        vec![Problem::UnknownCharacter {
            character: "#".to_string()
        }],
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
    // well-formed top-level item (the mid-edit scenario: the user just
    // typed `#` and the editor pushes the text). The
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
        diags
            .iter()
            .any(|p| matches!(p, Problem::UnknownCharacter { character } if character == "#")),
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
            diags
                .iter()
                .all(|p| matches!(p, Problem::UnknownCharacter { .. })),
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
    // Dropped, the byte would leave a stream of two clean items, and the file
    // would look perfect to every tool while the user was staring at the `#`.
    // Exactly one diagnostic, and the two items around it still parse.
    let input = "a := io.csv(\"a.csv\")\n#\nb := io.csv(\"b.csv\")\n";
    let (elapsed, diags) = parse_with_timeout(input, HANG_DETECTION_BUDGET);
    assert_eq!(
        diags,
        vec![Problem::UnknownCharacter {
            character: "#".to_string()
        }],
        "the byte between two good items must be reported, and only it",
    );
    assert!(
        elapsed < HAPPY_PATH_BUDGET,
        "took {elapsed:?}; expected < {HAPPY_PATH_BUDGET:?}",
    );
}
