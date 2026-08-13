//! Repo-wide guard: no committed `.snap` may carry an `assertion_line:` header.
//!
//! # What this proves
//!
//! That every snapshot in the repository went through `insta`'s own persistence
//! path. `insta::MetaData::trim_for_persistence` deletes `assertion_line`
//! **unconditionally** inside `Snapshot::save`, and `save` is what every
//! supported route to a committed snapshot goes through — `cargo insta accept`,
//! `cargo insta review`'s accept branch, and `INSTA_UPDATE=always`'s in-place
//! write. Only the `.snap.new` pending file keeps the field, because that one is
//! written by `save_new`, which does not trim.
//!
//! So an `assertion_line:` in a committed `.snap` is proof of exactly one thing:
//! somebody renamed a `.snap.new` by hand. That skips the review step entirely —
//! the whole point of a pending file is that a human looks at the diff before it
//! becomes the expectation — and it is invisible in a diff, because the field
//! looks like part of the format.
//!
//! Ten of the thirteen `fossil-hir` snapshots carried it when this guard was
//! written.
//!
//! # What this CANNOT prove
//!
//! - **That a snapshot was reviewed.** A hand-moved file with the line stripped
//!   passes. This catches the careless shortcut, not a determined one.
//! - **That a snapshot is correct**, or that it still corresponds to a live
//!   test. A `.snap` for a deleted test is stale, not malformed, and only
//!   `cargo insta test --unreferenced` finds those.
//! - **Anything about `.snap.new` files**, which legitimately carry the field.
//!   They are gitignored; if one is committed, that is a different guard's job.
//!
//! # Why it lives in `xtask`
//!
//! `xtask` is the one crate whose subject is the repository rather than a layer
//! of the compiler, and it is the only one that can walk `crates/*` without
//! inventing a dependency. It also depends on nothing that moves: this guard
//! stays green while the parser is being rewritten and every downstream crate is
//! red.
//!
//! There is no configuration alternative to this test. Verified against insta
//! 1.47.2: no setting, env var or `insta.yaml` key controls whether the field is
//! persisted — `trim_for_persistence` is not conditional on anything.

use std::path::{Path, PathBuf};

/// The repository root — `crates/xtask/` up two.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/xtask is two levels below the repo root")
        .to_path_buf()
}

/// Every `*.snap` under `dir`, skipping build output and vendored trees.
fn snapshots(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `target` and `node_modules` hold copies nobody commits; a dotdir
            // holds `.git`, whose objects are not text.
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            snapshots(&path, out);
        } else if path.extension().is_some_and(|e| e == "snap") {
            out.push(path);
        }
    }
}

#[test]
fn no_committed_snapshot_carries_an_assertion_line() {
    let root = repo_root();
    let mut files = Vec::new();
    snapshots(&root, &mut files);

    assert!(
        !files.is_empty(),
        "found no `.snap` files under {}, so this guard is asserting nothing — \
         the walk is broken, not the repository",
        root.display()
    );

    let offenders: Vec<String> = files
        .iter()
        .filter(|f| {
            std::fs::read_to_string(f)
                .is_ok_and(|text| text.lines().any(|l| l.starts_with("assertion_line:")))
        })
        .map(|f| {
            f.strip_prefix(&root)
                .unwrap_or(f)
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these snapshots carry an `assertion_line:` header, which insta removes \
         in `MetaData::trim_for_persistence` on every supported write — so they \
         were moved from `.snap.new` by hand and never reviewed:\n  {}\n\
         Re-accept them through `cargo insta review` (reading the diff), or \
         delete the line if the content is right.",
        offenders.join("\n  ")
    );
}
