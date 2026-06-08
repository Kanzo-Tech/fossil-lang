//! Repository automation. Run via `cargo xtask <command>` (alias in `.cargo/config.toml`).
//!
//! Commands:
//!   wasm-check   `cargo check --target wasm32-unknown-unknown` for every workspace
//!                crate in the dependency closure of the WASM cdylib crates. The set
//!                is DERIVED from the resolved graph — there is no hand-maintained
//!                list to drift (the old hardcoded `-p …` lists in `ci.yml` and the
//!                `.cargo` alias had already diverged: 9 crates vs 6).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::process::{Command, exit};

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("wasm-check") => wasm_check(),
        other => {
            if let Some(c) = other {
                eprintln!("xtask: unknown command {c:?}");
            }
            eprintln!("usage: cargo xtask wasm-check");
            exit(2);
        }
    }
}

fn wasm_check() {
    let crates = wasm_closure();
    eprintln!(
        "xtask: wasm-checking {} crate(s): {}",
        crates.len(),
        crates.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["check", "--target", "wasm32-unknown-unknown"]);
    for c in &crates {
        cmd.arg("-p").arg(c);
    }
    let status = cmd.status().expect("spawn cargo check");
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}

/// The workspace crates reachable from the cdylib (WASM) crates in the resolved
/// dependency graph — exactly the set that compiles to wasm32.
fn wasm_closure() -> BTreeSet<String> {
    let meta = cargo_metadata();

    let ws: HashSet<&str> = meta["workspace_members"]
        .as_array()
        .expect("workspace_members")
        .iter()
        .map(|v| v.as_str().expect("member id"))
        .collect();

    // Workspace package id → name, and the cdylib roots.
    let mut id_name: HashMap<&str, &str> = HashMap::new();
    let mut roots: Vec<&str> = Vec::new();
    for p in meta["packages"].as_array().expect("packages") {
        let id = p["id"].as_str().expect("package id");
        if !ws.contains(id) {
            continue;
        }
        id_name.insert(id, p["name"].as_str().expect("package name"));
        let is_cdylib = p["targets"].as_array().is_some_and(|targets| {
            targets.iter().any(|t| {
                t["crate_types"]
                    .as_array()
                    .is_some_and(|cts| cts.iter().any(|c| c.as_str() == Some("cdylib")))
            })
        });
        if is_cdylib {
            roots.push(id);
        }
    }

    // Adjacency from the resolve graph, restricted to workspace members (we only
    // `-p`-check our own crates; external deps are pulled in transitively).
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in meta["resolve"]["nodes"].as_array().expect("resolve.nodes") {
        let id = n["id"].as_str().expect("node id");
        if !ws.contains(id) {
            continue;
        }
        let deps = n["dependencies"]
            .as_array()
            .expect("node dependencies")
            .iter()
            .filter_map(|d| d.as_str())
            .filter(|d| ws.contains(d))
            .collect();
        adj.insert(id, deps);
    }

    // BFS from the cdylib roots.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = roots.into_iter().collect();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(deps) = adj.get(id) {
            queue.extend(deps.iter().copied());
        }
    }

    let names: BTreeSet<String> = seen
        .iter()
        .filter_map(|id| id_name.get(id).map(|n| (*n).to_string()))
        .collect();
    if names.is_empty() {
        eprintln!("xtask: no cdylib (WASM) crates found in the workspace");
        exit(1);
    }
    names
}

fn cargo_metadata() -> serde_json::Value {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .output()
        .expect("run cargo metadata");
    if !out.status.success() {
        eprintln!(
            "xtask: cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        exit(1);
    }
    serde_json::from_slice(&out.stdout).expect("parse cargo metadata json")
}
