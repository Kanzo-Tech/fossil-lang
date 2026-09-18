//! Repo-wide guard: the manifest version string is written down once.
//!
//! # What regressed, and what it cost
//!
//! `fossil_sinks::manifest::GRAPHAR_VERSION` existed, was documented, and was
//! read by two writers out of seven. The other five — `fossil-graph`'s manifest
//! and executor fixtures, its `dump_fixture` example, and `fossil-mcp`'s
//! in-crate and integration fixtures — spelled the value as a bare literal, so
//! editing the constant would have changed the string three crates emit in
//! exactly none of them. A constant that its own tree ignores is not a
//! constant; it is a comment with a type.
//!
//! That mattered on a date this repository could see coming.
//! `/docs/design/corpus` holds an open decision about replacing the string —
//! the manifest opens with `GraphAr`'s own version while the page specifying it
//! declares the corpus non-conformant — and priced the change in write sites.
//! The price was wrong by five, in the direction that hides: the edit looks
//! like one line, compiles after one line, and leaves three crates emitting the
//! old string with no diagnostic anywhere.
//!
//! # What this proves
//!
//! **In `crates/`, the bare string literal that `GRAPHAR_VERSION` holds appears
//! exactly once: in the `pub const` that defines it.** Both halves are read off
//! the tree and neither is written down here:
//!
//! - The NEEDLE comes out of the constant's own declaration, parsed from
//!   `crates/fossil-sinks/src/manifest.rs`. **This file never spells the
//!   version string**, which is not a flourish: a guard that hard-coded it
//!   would match itself, would need a self-exemption to compensate, and would
//!   go stale on the very rename it exists to make safe. Rename the constant's
//!   value and this guard follows it with no edit.
//! - The FILES come out of `git ls-files`, so an untracked scratch file is
//!   nobody's writer and a new crate is covered the day it is added. No crate
//!   list, and no exemption list — see below for why there is none to keep.
//!
//! # Why a writer and an assertion are told apart by their quotes
//!
//! A site that PRODUCES a manifest writes the value: `version:
//! GRAPHAR_VERSION.to_string()`, and before this guard, the same field carried a
//! quoted literal of the value in the same position. The needle — the value
//! **with its quotes** — matches that, and a doc comment that showed it here
//! would match too, which is why this one does not show it.
//!
//! A site that CHECKS what came out writes a substring of YAML:
//! `assert!(yaml.contains("version: gar/v1"))`. That is a different string
//! literal; the needle does not match inside it, because the quote that opens
//! it is followed by `version: `, not by the value. So the assertions are not
//! exempted, they are **not matched** — which is the difference between this
//! guard and the hand-maintained allow-list this repository has rejected twice
//! (`/docs/design/discarded` carries the row). There is no list here to drift.
//!
//! And the assertions must keep their literals. `contains(&format!("version:
//! {GRAPHAR_VERSION}"))` passes whatever the constant says, which is to say it
//! asserts nothing at all about what the writer emitted. The constant's doc
//! comment states that split; this guard enforces only the half that is
//! mechanical.
//!
//! # What this CANNOT prove
//!
//! - **That every writer reads the constant.** It proves no writer spells the
//!   value. `let v = "gar".to_string() + "/v1"` defeats it, as does reading the
//!   string from a fixture file. The defect it closes is the one that happened
//!   seven times, not every defect of its shape.
//! - **Anything outside `crates/`.** `apps/corpus`'s conformance manifests and
//!   `packages/corpus`'s fixtures carry the string as DATA — YAML on disk, the
//!   bytes a reader is handed — and a Rust constant cannot reach them. They are
//!   a separate problem with a separate answer, and this guard is silent on it
//!   rather than pretending to cover it.
//! - **That the string should be `gar/v1` at all.** That decision is open on
//!   `/docs/design/corpus` and this file has no opinion. What it does is make
//!   the decision cost one line instead of seven.
//!
//! # Why it lives in `xtask`
//!
//! Same reason as `engine_reach.rs`, `substrate_reach.rs` and
//! `tokio_placement.rs`: the claim is about the repository, not about any one
//! crate, so it cannot live in a crate that is part of what it measures.
//! `fossil-sinks` cannot assert that `fossil-mcp` does not spell a literal.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The file that declares the constant, relative to the repository root.
const DECLARATION: &str = "crates/fossil-sinks/src/manifest.rs";

/// The text that opens the declaration. Everything up to the opening quote of
/// the value — so this file names the constant without naming its value.
const DECLARATION_PREFIX: &str = "pub const GRAPHAR_VERSION: &str = ";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask sits two levels below the root")
        .to_path_buf()
}

/// The constant's value **with its quotes**, read out of the `pub const` line.
///
/// Returns the quoted form because that is what a bare-literal writer looks
/// like in Rust source, and what a YAML assertion does not.
fn quoted_version(root: &Path) -> String {
    let source = std::fs::read_to_string(root.join(DECLARATION))
        .unwrap_or_else(|e| panic!("{DECLARATION} is readable: {e}"));

    let line = source
        .lines()
        .find(|l| l.trim_start().starts_with(DECLARATION_PREFIX))
        .unwrap_or_else(|| {
            panic!(
                "{DECLARATION} no longer declares `{DECLARATION_PREFIX}…`. Either the constant \
                 moved — say where, here — or it was deleted, in which case this guard is \
                 measuring nothing and must go with it."
            )
        });

    let value = line
        .trim_start()
        .trim_start_matches(DECLARATION_PREFIX)
        .trim_end()
        .trim_end_matches(';')
        .trim();

    assert!(
        value.len() > 2 && value.starts_with('"') && value.ends_with('"'),
        "the declaration in {DECLARATION} is not a plain string literal ({value}), so the needle \
         cannot be derived from it",
    );
    value.to_string()
}

/// Every tracked `.rs` file under `crates/`, repository-relative.
fn tracked_rust_sources(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files", "--", "crates"])
        .current_dir(root)
        .output()
        .expect("git ls-files runs in the repository");
    assert!(
        out.status.success(),
        "git ls-files failed in {}",
        root.display(),
    );

    String::from_utf8(out.stdout)
        .expect("git ls-files emits UTF-8 paths")
        .lines()
        .filter(|p| Path::new(p).extension().is_some_and(|e| e == "rs"))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_manifest_version_string_is_spelled_in_one_place() {
    let root = repo_root();
    let needle = quoted_version(&root);

    let mut offenders: Vec<String> = Vec::new();
    for rel in tracked_rust_sources(&root) {
        let Ok(source) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        if !source.contains(&needle) {
            continue;
        }
        for (n, line) in source.lines().enumerate() {
            // The declaration itself is the one place the value belongs.
            if rel == DECLARATION && line.trim_start().starts_with(DECLARATION_PREFIX) {
                continue;
            }
            if line.contains(&needle) {
                offenders.push(format!("{rel}:{}  {}", n + 1, line.trim()));
            }
        }
    }
    offenders.sort();

    assert!(
        offenders.is_empty(),
        "the manifest version string is spelled as a literal outside its own declaration:\n\
         \x20 {}\n\n\
         A site that WRITES a manifest reads `fossil_sinks::manifest::GRAPHAR_VERSION` — a \
         fixture whose bytes stand in for a written corpus included, since the whole point of \
         the constant is that changing it changes what this tree emits. Five sites did not, and \
         the constant governed none of them.\n\
         A site that ASSERTS on emitted YAML keeps its literal and is not matched here: it \
         writes `contains(\"version: …\")`, whose quote is followed by the key, not the value. \
         Asserting against the constant instead would pass whatever the constant said.",
        offenders.join("\n  "),
    );
}

/// A guard over an empty set has stopped guarding. This one is two silent
/// failures away from that — a wrong root, or a `git ls-files` that returns
/// nothing — and would then pass forever over no files at all.
#[test]
fn the_guard_is_reading_the_repository() {
    let root = repo_root();

    assert!(
        root.join("Cargo.toml").is_file() && root.join("corpus.bnf").is_file(),
        "{} is not the workspace root, so the check above is reading the wrong tree",
        root.display(),
    );

    let sources = tracked_rust_sources(&root);
    assert!(
        sources.len() > 100,
        "only {} tracked Rust sources under crates/ — the walk is broken, not the tree",
        sources.len(),
    );
    assert!(
        sources.iter().any(|p| p == DECLARATION),
        "{DECLARATION} is not in the walk, so the needle's own home is unmeasured",
    );

    // The needle must be findable, or the check above searches for nothing.
    let needle = quoted_version(&root);
    let declaring = std::fs::read_to_string(root.join(DECLARATION)).expect("declaration readable");
    assert!(
        declaring.matches(&needle).count() == 1,
        "the value appears {} times in {DECLARATION}; the check above skips the declaration line \
         and would therefore report the rest, or none",
        declaring.matches(&needle).count(),
    );
}
