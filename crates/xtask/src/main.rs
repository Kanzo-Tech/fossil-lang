//! Repository automation. Run via `cargo xtask <command>` (alias in `.cargo/config.toml`).
//!
//! Commands:
//!   wasm-check   `cargo check --target wasm32-unknown-unknown` for every workspace
//!                crate in the dependency closure of the WASM cdylib crates. The set
//!                is DERIVED from the resolved graph — there is no hand-maintained
//!                list to drift (the old hardcoded `-p …` lists in `ci.yml` and the
//!                `.cargo` alias had already diverged: 9 crates vs 6).
//!   catalogue    Regenerate every projection of the catalogue: the provider
//!                statics from `catalogue.bnf`, and the reference page's tables
//!                from that file plus `fossil_hir::stdlib`'s registry. `--check`
//!                fails instead of writing, which is what CI runs.
//!   corpus       The same, one data file along: the writer's column table from
//!                `corpus.bnf`, projected into Rust and TypeScript. Two data
//!                files, two commands, one generator loop.
//!
//! What is generated from a Rust type rather than a data file — the JSON
//! Schemas and the TypeScript declarations — is `tests/wire.rs`, because the
//! types live in crates this binary must not link.

use std::process::{Command, exit};

use xtask::{catalogue, corpus, depgraph};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("wasm-check") => wasm_check(),
        Some("catalogue") => generate(
            "catalogue.bnf",
            "catalogue",
            catalogue::generated(),
            args.next().as_deref() == Some("--check"),
        ),
        Some("corpus") => generate(
            "corpus.bnf",
            "corpus",
            corpus::generated(),
            args.next().as_deref() == Some("--check"),
        ),
        other => {
            if let Some(c) = other {
                eprintln!("xtask: unknown command {c:?}");
            }
            eprintln!("usage: cargo xtask <wasm-check | catalogue [--check] | corpus [--check]>");
            exit(2);
        }
    }
}

/// Write — or, under `--check`, prove current — every file generated from one
/// data file.
///
/// One loop for every data file rather than one per command: what a generator
/// command does is identical and only the source and the fix-it line differ, so
/// a second copy of this would be a second place for the `--check` semantics to
/// drift. The check mode names the file and the command that fixes it, because
/// the failure a generator produces is read by somebody who did not run it.
fn generate(source: &str, command: &str, targets: Vec<(std::path::PathBuf, String)>, check: bool) {
    let root = catalogue::repo_root();
    let mut stale = Vec::new();
    for (path, want) in targets {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        if have == want {
            continue;
        }
        if check {
            stale.push(shown);
        } else {
            std::fs::create_dir_all(path.parent().expect("a file has a parent"))
                .expect("create the generated file's directory");
            std::fs::write(&path, want).expect("write the generated file");
            eprintln!("xtask: wrote {shown}");
        }
    }
    if !stale.is_empty() {
        eprintln!(
            "xtask: {} generated file(s) do not match `{source}`:",
            stale.len()
        );
        for s in &stale {
            eprintln!("  {s}");
        }
        eprintln!("run `cargo xtask {command}` and commit the result");
        exit(1);
    }
}

fn wasm_check() {
    let meta = depgraph::metadata(Some(WASM));
    let crates = depgraph::wasm_closure(&meta);
    eprintln!(
        "xtask: wasm-checking {} crate(s): {}",
        crates.len(),
        crates.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    // `tokio` never reaches a wasm build from our own manifests: a runtime is a
    // host's, and `tokio::time::sleep` panics on wasm32. The graph is the one
    // cargo resolved for wasm32, so a `cfg`-gated dependency is already gone.
    let holders = depgraph::direct_dependents(&meta, &crates, "tokio");
    if !holders.is_empty() {
        eprintln!(
            "xtask: these wasm crates depend on `tokio` on wasm32: {}; gate it behind \
             `cfg(not(target_arch = \"wasm32\"))` or move it to dev-dependencies",
            holders.into_iter().collect::<Vec<_>>().join(", ")
        );
        exit(1);
    }
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["check", "--target", WASM]);
    for c in &crates {
        cmd.arg("-p").arg(c);
    }
    let status = cmd.status().expect("spawn cargo check");
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}

const WASM: &str = "wasm32-unknown-unknown";
