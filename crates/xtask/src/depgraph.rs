//! The workspace dependency graph, as `cargo metadata` reports it.
//!
//! Two guards ask the same question of it and neither may keep its own copy of
//! the answer: `tests/engine_reach.rs` asks which crates link an execution
//! engine `deny.toml` bans, and `tests/substrate_reach.rs` asks whether the
//! batch pass links the Salsa substrate. Both questions are *which workspace
//! members reach this crate over normal edges*, and a walk copied into both
//! files is the second copy this repository has spent two deletions learning to
//! avoid. It lives in the library rather than in one of the test binaries
//! because an integration test is its own crate and cannot `use` its sibling.
//!
//! What is deliberately NOT here is the wasm closure. `tests/tokio_placement.rs`
//! re-derives that one on purpose, from the same inputs `cargo xtask wasm-check`
//! uses, so that two independent answers to "what is a wasm build" stay honest.
//! Sharing is for machinery, not for verdicts.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::process::Command;

use serde_json::Value;

/// Which dependency edges count as LINKING.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edges {
    /// Normal edges only — the dependency ends up inside the crate's own
    /// artefact.
    Linking,
    /// Any edge, including dev and build. Used to ask whether a name still has
    /// any relationship at all with the crate that declares it.
    Any,
}

/// `cargo metadata`'s spelling of a normal dependency is a `null` kind.
fn is_normal_edge(dep: &Value) -> bool {
    dep["dep_kinds"]
        .as_array()
        .is_some_and(|ks| ks.iter().any(|k| k["kind"].is_null()))
}

/// Every workspace member from which `dependency` is reachable over `edges`,
/// excluding a crate reaching itself.
///
/// The walk is over the WHOLE resolve graph, not just workspace members: the
/// interesting paths are precisely the ones that leave the workspace and come
/// back (`fossil-lsp` → `fossil-introspect` → `duckdb`, where the middle hop is
/// a member, and equally a path through a registry crate).
///
/// # Panics
///
/// If `meta` is not `cargo metadata --format-version 1` output.
pub fn reachers(meta: &Value, dependency: &str, edges: Edges) -> BTreeSet<String> {
    let mut id_name: HashMap<&str, &str> = HashMap::new();
    for p in meta["packages"].as_array().expect("packages") {
        id_name.insert(
            p["id"].as_str().expect("package id"),
            p["name"].as_str().expect("package name"),
        );
    }

    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in meta["resolve"]["nodes"].as_array().expect("resolve.nodes") {
        let id = n["id"].as_str().expect("node id");
        adj.insert(
            id,
            n["deps"]
                .as_array()
                .expect("node deps")
                .iter()
                .filter(|d| edges == Edges::Any || is_normal_edge(d))
                .filter_map(|d| d["pkg"].as_str())
                .collect(),
        );
    }

    let mut out = BTreeSet::new();
    for member in workspace_ids(meta) {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = adj.get(member).cloned().unwrap_or_default().into();
        let mut found = false;
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            if id_name.get(id).copied() == Some(dependency) {
                found = true;
                break;
            }
            if let Some(deps) = adj.get(id) {
                queue.extend(deps.iter().copied());
            }
        }
        if found {
            out.insert((*id_name.get(member).expect("member name")).to_string());
        }
    }
    out
}

/// The package ids of every workspace member.
///
/// # Panics
///
/// If `meta` is not `cargo metadata --format-version 1` output.
pub fn workspace_ids(meta: &Value) -> BTreeSet<&str> {
    meta["workspace_members"]
        .as_array()
        .expect("workspace_members")
        .iter()
        .map(|v| v.as_str().expect("member id"))
        .collect()
}

/// The names of every workspace member.
///
/// # Panics
///
/// If `meta` is not `cargo metadata --format-version 1` output.
pub fn member_names(meta: &Value) -> BTreeSet<String> {
    let ws = workspace_ids(meta);
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|p| ws.contains(p["id"].as_str().expect("package id")))
        .map(|p| p["name"].as_str().expect("package name").to_string())
        .collect()
}

/// `cargo metadata --format-version 1`, run at the repository root.
///
/// # Panics
///
/// If cargo cannot be run, fails, or emits something that is not the JSON this
/// module reads — in every case the guard calling this cannot see the workspace
/// at all, and saying so loudly beats reporting an empty graph as agreement.
pub fn metadata() -> Value {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .current_dir(crate::catalogue::repo_root())
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed, so this guard cannot see the workspace at all:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("parse cargo metadata json")
}

// ------------------------------------------- the failure modes, each proved
//
// The guards over this walk are green once the tree is right, which is exactly
// when a walk stops demonstrating that it works. These feed it inputs that
// should fail, so every branch above is known to fire.

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolve graph: `edges` is `(from, to, kind)` where `kind` is `None`
    /// for a normal dependency.
    fn graph(members: &[&str], edges: &[(&str, &str, Option<&str>)]) -> Value {
        let names: BTreeSet<&str> = members
            .iter()
            .copied()
            .chain(edges.iter().flat_map(|(a, b, _)| [*a, *b]))
            .collect();
        let packages: Vec<Value> = names
            .iter()
            .map(|n| serde_json::json!({ "id": *n, "name": *n }))
            .collect();
        let nodes: Vec<Value> = names
            .iter()
            .map(|n| {
                let deps: Vec<Value> = edges
                    .iter()
                    .filter(|(a, _, _)| a == n)
                    .map(|(_, b, k)| {
                        serde_json::json!({
                            "pkg": *b,
                            "dep_kinds": [{ "kind": k.map(str::to_string) }],
                        })
                    })
                    .collect();
                serde_json::json!({ "id": *n, "deps": deps })
            })
            .collect();
        serde_json::json!({
            "packages": packages,
            "workspace_members": members,
            "resolve": { "nodes": nodes },
        })
    }

    #[test]
    fn a_transitive_normal_edge_is_a_link() {
        // The exact shape `cargo deny check bans` misses: shell -> middle -> engine.
        let g = graph(
            &["shell", "middle"],
            &[("shell", "middle", None), ("middle", "engine", None)],
        );
        assert_eq!(
            reachers(&g, "engine", Edges::Linking),
            ["middle".to_string(), "shell".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "a crate two hops from the dependency links it just as hard as its parent"
        );
    }

    // The two expected sets in this test are read side by side — one crate under
    // `Linking`, two under `Any` — and that comparison is the test. Writing the
    // first as `std::iter::once` would hide it behind a different construction.
    #[allow(clippy::iter_on_single_items)]
    #[test]
    fn a_dev_edge_is_not_a_link_and_nothing_behind_it_is_either() {
        // `fossil-layout`'s shape under `salsa` since `39d0fb8`, exactly: the
        // pass keeps `fossil-df` for a bench baseline and the substrate behind
        // it reaches no artefact of the pass.
        let g = graph(
            &["cmd", "harness"],
            &[("cmd", "harness", Some("dev")), ("harness", "engine", None)],
        );
        assert_eq!(
            reachers(&g, "engine", Edges::Linking),
            ["harness".to_string()].into_iter().collect::<BTreeSet<_>>(),
            "`harness` links the engine and `cmd` does not: a dev-dependency puts \
             it in a test binary, not in the crate, and the dev edge's own normal \
             deps do not reach the parent's artefact either"
        );
        assert_eq!(
            reachers(&g, "engine", Edges::Any),
            ["cmd".to_string(), "harness".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "`Any` is what keeps a legitimately dev-only wrapper from reading as stale"
        );
    }

    #[test]
    fn a_direct_dev_edge_to_the_dependency_is_not_a_link() {
        let g = graph(&["engine-user"], &[("engine-user", "engine", Some("dev"))]);
        assert!(reachers(&g, "engine", Edges::Linking).is_empty());
        assert_eq!(reachers(&g, "engine", Edges::Any).len(), 1);
    }

    #[test]
    fn a_cycle_does_not_hang_the_walk() {
        let g = graph(
            &["a"],
            &[("a", "b", None), ("b", "c", None), ("c", "b", None)],
        );
        assert!(reachers(&g, "engine", Edges::Linking).is_empty());
    }

    #[test]
    fn a_crate_is_not_its_own_reacher() {
        let g = graph(&["engine"], &[]);
        assert!(
            reachers(&g, "engine", Edges::Linking).is_empty(),
            "the depended-on crate is not a workspace member here, but if it ever \
             were, it must not be required to declare itself"
        );
    }
}
