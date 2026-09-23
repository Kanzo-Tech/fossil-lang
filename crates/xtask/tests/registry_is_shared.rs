//! **Production code shares one stdlib catalogue; it does not build its own.**
//!
//! `FunctionRegistry::stdlib_default` is the CONSTRUCTOR — it walks every row,
//! its signature and its lowering, and hands back a fresh `HashMap`.
//! `fossil_hir::stdlib::stdlib` is the shared one, behind a `LazyLock` that
//! exists for exactly this. The two names differ by eight characters, both
//! return the same type, and picking the wrong one is not a type error, a
//! clippy lint or a visible failure — it is a rebuild of the whole catalogue
//! per call.
//!
//! It had already happened. `fossil-ide`'s completion entry point called the
//! constructor, so every completion request in the editor rebuilt the table,
//! on the keystroke path, having walked past the `LazyLock` two crates away.
//! Nothing was red. The measurement that would have shown it — a count of
//! constructions rather than a wall clock — did not exist either.
//!
//! # What this proves and what it does not
//!
//! It reads the tree as text, which is the shape `packages/introspect`'s
//! parity test uses and the one this repo prefers to a `///` that asserts an
//! invariant nothing checks: derive the guard from the original instead of
//! repeating it.
//!
//! - It proves no `src/` file outside the catalogue's own module names the
//!   constructor. **Tests are exempt on purpose**: a test that wants a fresh,
//!   unshared registry is asking a legitimate question, and thirteen of them do.
//! - It does NOT prove the shared one is reached cheaply, only that the
//!   expensive one is not reached at all. A future `stdlib().clone()` would
//!   pass this and cost the same.
//! - It does NOT prove anything about `Registry` — the `io.` half, which is a
//!   Salsa input and has its own question (durability), not this one.

use std::path::{Path, PathBuf};

/// The constructor's name, written once.
const CONSTRUCTOR: &str = "stdlib_default";

/// Where the constructor legitimately appears in production code: its own
/// definition and the `LazyLock` that calls it.
const HOME: &str = "crates/fossil-hir/src/stdlib.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/xtask has two ancestors")
        .to_path_buf()
}

/// Every `.rs` under `crates/*/src/` that is not a test module.
///
/// «`tests/` is a sibling of `src/`, not a child» is the obvious rule and it is
/// WRONG here: `crates/fossil-hir/src/stdlib/tests.rs` is a unit-test module
/// living inside `src/`, and the first run of this guard flagged its thirteen
/// legitimate calls. A `tests.rs` under `src/` is excluded by name.
///
/// The limitation that leaves, stated rather than hidden: an inline
/// `#[cfg(test)] mod tests { … }` in an ordinary source file would still be
/// flagged. That is the safe direction — a false positive is a failing test
/// somebody reads, and a false negative is the defect this exists to catch —
/// but it is a reason to keep test modules in their own file, which this tree
/// already does.
fn production_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.components().any(|c| c.as_os_str() == "src")
                && path.file_stem().is_some_and(|n| n != "tests")
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn no_production_file_builds_its_own_stdlib_catalogue() {
    let root = repo_root();
    let home = root.join(HOME);
    let mut offenders = Vec::new();

    for path in production_sources(&root) {
        if path == home {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            // A call, not a mention. The prose that explains the distinction
            // has to be able to name the thing it is warning about.
            if line.contains(&format!("{CONSTRUCTOR}()")) && !line.trim_start().starts_with("//") {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(&root).unwrap_or(&path).display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these build their own stdlib catalogue instead of sharing \
         `fossil_hir::stdlib::stdlib()`:\n  {}\n\n\
         The constructor walks every row; the shared one is a `LazyLock`. If a \
         fresh registry is genuinely wanted here, this belongs in a test.",
        offenders.join("\n  ")
    );
}

/// The guard is worthless if its own search finds nothing to search.
///
/// `no_production_file_builds_its_own_stdlib_catalogue` passes vacuously if the
/// walk is broken, the path is wrong or the constructor is renamed — the same
/// failure `xtask`'s catalogue test names for its own parse.
#[test]
fn the_search_actually_reads_the_tree() {
    let root = repo_root();
    let files = production_sources(&root);
    assert!(
        files.len() > 100,
        "expected the production sources of two dozen crates, found {}",
        files.len()
    );
    let home = std::fs::read_to_string(root.join(HOME)).expect("read the catalogue module");
    assert!(
        home.contains(&format!("{CONSTRUCTOR}()")) || home.contains(CONSTRUCTOR),
        "`{HOME}` no longer names `{CONSTRUCTOR}`, so the guard above is \
         searching for something that does not exist"
    );
}
