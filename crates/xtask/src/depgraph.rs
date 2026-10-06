//! The workspace dependency graph, as `cargo metadata` reports it.
//!
//! `cargo xtask wasm-check` derives the wasm closure from it, and
//! `tests/engine_reach.rs` asks which crates link an execution engine
//! `deny.toml` bans — *which workspace members reach this crate over normal
//! edges*.

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

/// The workspace crates reachable over normal edges from the cdylib (WASM)
/// crates, as `metadata(Some("wasm32-unknown-unknown"))` resolves them — so
/// cargo, not a string match, decides which `cfg`-gated edges a wasm build
/// has. Nothing here names a crate: a crate a cdylib comes to depend on joins
/// the gate, and one it stops depending on leaves.
///
/// Normal edges only: a dev-dependency is built for a test on the host, never
/// for wasm32. It put `fossil-introspect` (and `DuckDB` under it) in the gate
/// the day `fossil-df`'s tests took it.
///
/// # Panics
///
/// If `meta` is not `cargo metadata --format-version 1` output.
pub fn wasm_closure(meta: &Value) -> BTreeSet<String> {
    let ws = workspace_ids(meta);
    let mut id_name: HashMap<&str, &str> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for p in meta["packages"].as_array().expect("packages") {
        let id = p["id"].as_str().expect("package id");
        if !ws.contains(id) {
            continue;
        }
        id_name.insert(id, p["name"].as_str().expect("package name"));
        let cdylib = p["targets"].as_array().is_some_and(|ts| {
            ts.iter().any(|t| {
                t["crate_types"]
                    .as_array()
                    .is_some_and(|cs| cs.iter().any(|c| c.as_str() == Some("cdylib")))
            })
        });
        if cdylib {
            queue.push_back(id);
        }
    }
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in meta["resolve"]["nodes"].as_array().expect("resolve.nodes") {
        let deps = n["deps"].as_array().expect("node deps").iter();
        adj.insert(
            n["id"].as_str().expect("node id"),
            deps.filter(|d| is_normal_edge(d))
                .filter_map(|d| d["pkg"].as_str())
                .filter(|d| ws.contains(d))
                .collect(),
        );
    }
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some(id) = queue.pop_front() {
        if seen.insert(id) {
            queue.extend(adj.get(id).into_iter().flatten().copied());
        }
    }
    seen.iter().map(|id| id_name[id].to_string()).collect()
}

/// The members of `closure` that depend on `dependency` directly over a normal
/// edge, in the graph `meta` resolved.
///
/// # Panics
///
/// If `meta` is not `cargo metadata --format-version 1` output.
pub fn direct_dependents(
    meta: &Value,
    closure: &BTreeSet<String>,
    dependency: &str,
) -> BTreeSet<String> {
    let name_of: HashMap<&str, &str> = meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            (
                p["id"].as_str().expect("id"),
                p["name"].as_str().expect("name"),
            )
        })
        .collect();
    meta["resolve"]["nodes"]
        .as_array()
        .expect("resolve.nodes")
        .iter()
        .filter(|n| closure.contains(name_of[n["id"].as_str().expect("node id")]))
        .filter(|n| {
            n["deps"].as_array().expect("node deps").iter().any(|d| {
                is_normal_edge(d) && d["pkg"].as_str().map(|p| name_of[p]) == Some(dependency)
            })
        })
        .map(|n| name_of[n["id"].as_str().expect("node id")].to_string())
        .collect()
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

/// `cargo metadata --format-version 1`, run at the repository root and, given
/// a `platform`, resolved for that target only (`--filter-platform`).
///
/// # Panics
///
/// If cargo cannot be run, fails, or emits something that is not the JSON this
/// module reads — in every case the guard calling this cannot see the workspace
/// at all, and saying so loudly beats reporting an empty graph as agreement.
pub fn metadata(platform: Option<&str>) -> Value {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["metadata", "--format-version", "1"]);
    if let Some(target) = platform {
        cmd.args(["--filter-platform", target]);
    }
    let out = cmd
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
        // A crate that keeps another for a test only: the engine behind the
        // dev edge reaches no artefact of the crate.
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
