//! Repo-wide guard: the Rust half of `/docs/design/failure`'s guards, over
//! every `crates/*/src` file. The TypeScript half is
//! `packages/types/tests/failure-guards.test.ts`.
//!
//! # What this holds
//!
//! 1. **No `let _ = … .await` without a reason.** Awaiting a future and
//!    discarding its `Result` is how a failure is lost on purpose; it needs a
//!    `//` comment on the line or the line before. The audit found one —
//!    `let _ = upload.abort().await` — and an orphaned multipart upload costs
//!    money with nobody told.
//! 2. **Every `object_store` builder is given client options and a retry
//!    policy.** Between a builder's `::new()` and its `.build()` there is a
//!    `.with_client_options(` and a `.with_retry(`, so no store fossil builds
//!    waits on the network with `reqwest`'s defaults — which is no timeout.
//!
//! # Why
//!
//! `/docs/design/failure`'s G1 and G2 are prose, and the audit of 2026-10-01
//! counted ten waits with no end and fifteen failures nobody saw in a tree
//! whose reviewers had read that prose.
//!
//! # What this cannot prove
//!
//! - **That the reason is true** — any comment satisfies rule 1 — or a failure
//!   discarded another way: `.ok();`, `drop(…)`, a `match` arm that ignores it.
//! - **The figures themselves**, nor that they reach the request: on wasm32
//!   `object_store` applies none of the timeouts, which is why `fossil-storage`
//!   races each request itself there. Rule 2 reads text between `::new()` and
//!   `.build()`; a builder assembled across functions escapes it.

use std::fs;
use std::path::{Path, PathBuf};

const BUILDERS: &[&str] = &[
    "AmazonS3Builder::new()",
    "MicrosoftAzureBuilder::new()",
    "HttpBuilder::new()",
    "GoogleCloudStorageBuilder::new()",
];

fn sources() -> Vec<(PathBuf, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = fs::read_dir(&crates)
        .expect("crates/")
        .filter_map(|e| Some(e.ok()?.path().join("src")))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("a source directory").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = fs::read_to_string(&path).expect("a source file");
                out.push((path, text));
            }
        }
    }
    out
}

fn shown(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.split("/crates/").last().unwrap_or(&s).to_string()
}

#[test]
fn no_awaited_result_is_discarded_without_a_reason() {
    let files = sources();
    assert!(
        files
            .iter()
            .any(|(p, _)| p.ends_with("fossil-storage/src/store.rs"))
    );
    let mut bare = Vec::new();
    for (path, text) in &files {
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            if !(code.trim_start().starts_with("let _ =") && code.contains(".await")) {
                continue;
            }
            let reasoned =
                line.contains("//") || i > 0 && lines[i - 1].trim_start().starts_with("//");
            if !reasoned {
                bare.push(format!("{}:{}: {}", shown(path), i + 1, line.trim()));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "a discarded await with no reason:\n{}",
        bare.join("\n")
    );
}

#[test]
fn every_object_store_builder_sets_client_options_and_retry() {
    let mut unset = Vec::new();
    let mut seen = 0;
    for (path, text) in sources() {
        for builder in BUILDERS {
            for (at, _) in text.match_indices(builder) {
                seen += 1;
                let rest = &text[at..];
                let chain = &rest[..rest.find(".build()").unwrap_or(rest.len())];
                for needed in [".with_client_options(", ".with_retry("] {
                    if !chain.contains(needed) {
                        let line = text[..at].lines().count();
                        unset.push(format!(
                            "{}:{line}: {builder} without {needed}",
                            shown(&path)
                        ));
                    }
                }
            }
        }
    }
    assert!(
        seen >= 3,
        "found only {seen} builders — the scan is reading the wrong tree"
    );
    assert!(
        unset.is_empty(),
        "a store built without its client options:\n{}",
        unset.join("\n")
    );
}
