//! Shared test support for the `fossil-cli` integration tests.
//!
//! Every test in this directory spawns the `fossil` binary against a working
//! directory it seeds itself; this is the one place that decides where that
//! directory lives.

use std::path::PathBuf;

/// A fresh, empty working directory under `std::env::temp_dir()`, named
/// `<prefix>-<test_name>-<pid>`.
///
/// **The `<pid>` is load-bearing.** These workdirs are wiped with
/// `remove_dir_all` before they are seeded, so a path that depends only on the
/// test name is shared by every `fossil-cli` test process on the machine — and
/// two concurrent `cargo test` runs over the same checkout then delete each
/// other's workdir mid-run. The symptom is a test that fails with no message
/// and passes when rerun alone, in whichever process lost the race; nothing in
/// the failure points at the other run. Measured on 2026-08-10: three
/// `run_rdf` tests died that way against a parallel `cargo test --workspace`.
/// Keep the path unique per process, or that bug comes back invisible.
///
/// The wipe stays even though the pid already makes the path fresh: pids are
/// reused, and a reused one must not inherit the previous run's artefacts.
///
/// Nothing removes the directory afterwards — a failed run leaves its parquet,
/// program and manifest on disk to be read.
pub(crate) fn unique_workdir(prefix: &str, test_name: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("{prefix}-{test_name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create workdir");
    tmp
}
