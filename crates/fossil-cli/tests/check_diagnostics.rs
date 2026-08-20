//! `fossil check` golden-output test.
//!
//! Invokes the `fossil` binary on a deliberately-broken fixture — a
//! `users.naem` column typo against a source whose header is `id,name,age`,
//! which triggers the did-you-mean diagnostic — captures stderr, strips
//! ANSI color codes for a stable snapshot, and asserts:
//!   1. the process exits non-zero (an error diagnostic was accumulated);
//!   2. the rendered text shows a source-span label (`here`) AND a `help:`
//!      did-you-mean line suggesting the `name` column.
//!
//! `NO_COLOR=1` neutralises miette's `GraphicalReportHandler` color so the
//! snapshot is stable across terminals/CI; a defensive ANSI strip handles any
//! residual escapes.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Locate the repo root from `CARGO_MANIFEST_DIR` (= `.../crates/fossil-cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR has at least two parents")
        .to_path_buf()
}

/// The `fossil` binary this test drives — cargo's own path for it.
///
/// **It used to shell out to `cargo build` and then hard-code
/// `<repo>/target/debug/fossil`**, which is a test that can pass against a
/// binary it did not build: with `CARGO_TARGET_DIR` set — which is how this
/// repository's own instructions say to drive the suite — the build lands
/// elsewhere and that path holds whatever was left there last. Measured on
/// 2026-08-13: the file at the hard-coded path was **29 hours old**, older than
/// the parser rewrite, the provider registry, `@rename` and the edge
/// constructor. Everything this file reported that day was about a compiler
/// nobody had edited.
///
/// `CARGO_BIN_EXE_<name>` is cargo's answer: it is set for an integration test
/// and points at the binary of THIS build, which cargo has already built before
/// the test runs. No path to guess, and no `cargo build` spawned from inside a
/// test — the same fix `crates/fossil-lsp/tests/` took.
fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| PathBuf::from(env!("CARGO_BIN_EXE_fossil")))
}

/// Strip ANSI SGR escape sequences (`ESC [ ... m`) so the snapshot is stable
/// regardless of color support. A tiny hand-rolled scanner avoids a new dep.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Consume up to and including the final byte of the escape.
            if chars.peek() == Some(&'[') {
                chars.next();
                for e in chars.by_ref() {
                    if e.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn check_broken_field_renders_span_and_help_and_exits_nonzero() {
    let bin = fossil_binary();
    let fixture = repo_root()
        .join("crates")
        .join("fossil-cli")
        .join("tests")
        .join("fixtures")
        .join("broken_field")
        .join("mapping.fossil");

    let output = Command::new(bin)
        .args(["check", fixture.to_str().expect("utf8 fixture path")])
        .env("NO_COLOR", "1")
        // Silence tracing. Every log line goes to stderr, and the snapshot below
        // is OF stderr — a timestamped line made this test unpassable, because
        // every run produced text the golden file could never match.
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn fossil check");

    // 1. Non-zero exit: an error diagnostic was accumulated.
    assert!(
        !output.status.success(),
        "fossil check on a broken fixture must exit non-zero; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));

    // 2. The render must visibly show a span label + the did-you-mean help.
    assert!(
        stderr.contains("unknown column `naem`"),
        "diagnostic headline missing; got:\n{stderr}"
    );
    assert!(
        stderr.contains("did you mean `name`"),
        "did-you-mean help line missing; got:\n{stderr}"
    );
    assert!(
        stderr.contains("help:"),
        "rustc-style help: line missing; got:\n{stderr}"
    );
    assert!(
        stderr.contains("here"),
        "source-span label missing; got:\n{stderr}"
    );

    // Golden-output snapshot of the ANSI-stripped render. Filter the absolute
    // fixture path (varies per machine) to a stable token so the snapshot is
    // portable.
    let normalised = stderr.replace(
        fixture.to_str().expect("utf8 fixture path"),
        "<FIXTURE>/mapping.fossil",
    );
    insta::assert_snapshot!(normalised);
}

/// Happy path: `fossil check` on the canonical clean program exits zero and
/// prints `ok`. Runs from the repo root so the `io.csv("examples/users.csv")`
/// binding resolves for pre-introspection; `check` writes nothing.
#[test]
fn check_hello_exits_zero_and_prints_ok() {
    let bin = fossil_binary();
    let output = Command::new(bin)
        .args(["check", "examples/hello.fossil"])
        .current_dir(repo_root())
        .output()
        .expect("spawn fossil check");

    assert!(
        output.status.success(),
        "fossil check on a clean program must exit zero; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ok"),
        "fossil check should print 'ok'; got: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    // …and it must not have GAINED one. The file-level drain added for the
    // no-mapping case runs on a different branch than this program takes; if it
    // ever ran here it would republish whatever `parse` accumulated, and a
    // clean file would start reporting.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("declares no mapping"),
        "hello.fossil declares a mapping; got: {stdout}"
    );
    assert!(
        !strip_ansi(&String::from_utf8_lossy(&output.stderr)).contains("Error"),
        "a clean program must produce no diagnostic; got:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Write `text` to a `.fossil` in this test's own workdir and run `fossil check`
/// on it. Each case names its own directory: the CLI reads relative source paths
/// against the program's parent, and two tests sharing one is two tests sharing
/// a state.
fn check_text(test_name: &str, text: &str) -> std::process::Output {
    let dir = common::unique_workdir("fossil-check", test_name);
    let file = dir.join("subject.fossil");
    std::fs::write(&file, text).expect("write subject");
    Command::new(fossil_binary())
        .args(["check", file.to_str().expect("utf8 path")])
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn fossil check")
}

/// An EMPTY file: zero mappings, zero parse errors. Deliberately a success —
/// nothing in it is wrong — but it must not be rendered as a clean program,
/// because `fossil run` refuses it (`no mapping found`). The distinct line is
/// the whole point: `ok — no errors` on a file that builds nothing is the wrong
/// answer even when the exit code is right.
#[test]
fn check_empty_file_exits_zero_and_says_it_declares_no_mapping() {
    let output = check_text("empty", "");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "an empty file has no error in it; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.contains("declares no mapping"),
        "an empty file must not read as a checked program; got: {stdout}"
    );
}

/// A WHOLLY-UNPARSEABLE file: zero mappings, and parse errors the old drain
/// could not reach. Before the file-level drain this printed `ok — no errors`
/// and exited 0 — a wrong answer on the surface a downstream product invokes.
#[test]
fn check_unparseable_file_exits_nonzero_and_reports_the_parse_error() {
    let output = check_text("unparseable", "!@#$%^&*() )))\n{{{ ]]]\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "garbage is not a program; stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stdout.contains("ok —"),
        "a file that does not parse must not be reported as ok; got: {stdout}"
    );
    assert!(
        stderr.to_lowercase().contains("unexpected"),
        "the parse error must reach stderr; got:\n{stderr}"
    );
}

/// The double-report guard. `def_map` sits in EVERY mapping's dependency
/// subtree, so draining it alongside the per-mapping loop would publish each
/// parse error twice. One mapping, one parse error, one line about it.
///
/// The break is a property written without its `=`. It is chosen because it is
/// the only thing wrong with the file and the parser recovers from it cleanly —
/// exactly ONE `ParseDiagnostic` comes out of `parse`, so a second line about it
/// on stderr can only have been published twice. The old fixture broke a
/// `prefix` line, which the current parser answers with a retired-spelling
/// refusal and a cascade behind it; a fixture that produces thirty-four errors
/// cannot say anything about how many times one of them is printed.
///
/// The count is `assert_eq!`, never `>= 1`: "at least once" is the assertion
/// this test would pass with the bug it exists to catch.
#[test]
fn a_parse_error_in_a_file_with_a_mapping_is_reported_once() {
    let output = check_text(
        "parse-error-once",
        concat!(
            "type { Person } := io.shex(\"person.shex\")\n",
            "\n",
            "users := io.csv(\"users.csv\")\n",
            "\n",
            "User : Person from users\n",
            "    @subject = \"https://example.org/user/{users.id}\"\n",
            "    name users.name\n",
        ),
    );
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));

    assert!(!output.status.success(), "the file does not parse");
    let occurrences = stderr.matches("expected ASSIGN, found IDENT").count();
    assert_eq!(
        occurrences, 1,
        "the parse error must be reported exactly once; got {occurrences} in:\n{stderr}"
    );
}
