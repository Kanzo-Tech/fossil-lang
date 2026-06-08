//! `fossil check` golden-output test (CLI-02 / SC#1).
//!
//! Invokes the `fossil` binary on a deliberately-broken fixture — a `.naem`
//! field typo against a CSVW-described source whose columns are `id`/`name`,
//! which triggers the Phase-3 did-you-mean diagnostic — captures stderr, strips
//! ANSI color codes for a stable snapshot, and asserts:
//!   1. the process exits non-zero (an error diagnostic was accumulated);
//!   2. the rendered text shows a source-span label (`here`) AND a `help:`
//!      did-you-mean line suggesting the `name` column.
//!
//! `NO_COLOR=1` neutralises miette's `GraphicalReportHandler` color so the
//! snapshot is stable across terminals/CI; a defensive ANSI strip handles any
//! residual escapes.

#![cfg(not(target_arch = "wasm32"))]

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

/// Build the `fossil` binary once per test process.
fn fossil_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "fossil-cli", "--bin", "fossil"])
            .status()
            .expect("spawn cargo build");
        assert!(status.success(), "cargo build -p fossil-cli failed");
        let bin = repo_root().join("target").join("debug").join("fossil");
        assert!(bin.exists(), "fossil binary missing at {}", bin.display());
        bin
    })
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
}
