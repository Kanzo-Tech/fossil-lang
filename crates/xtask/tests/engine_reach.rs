//! Every workspace crate that LINKS a banned engine is in that engine's
//! `wrappers` list in `deny.toml`.
//!
//! `cargo deny check bans` matches `wrappers` against the banned crate's DIRECT
//! parents only, so a crate that reaches `duckdb` or `datafusion` one hop away
//! passes it. This takes the transitive closure over normal edges (a
//! dev-dependency links the engine into a test binary, not the crate), from
//! `cargo metadata` through `xtask::depgraph`, against the lists `deny.toml`
//! already holds. A ban with no `wrappers` is an outright strike and is skipped.

use std::collections::{BTreeMap, BTreeSet};

use xtask::depgraph::{self, Edges};

// ------------------------------------------------------------------ the policy

/// The `wrappers` list of every `[bans].deny` entry that has one, keyed by the
/// banned crate.
///
/// An entry without `wrappers` is not an engine ring — it is an outright ban
/// (`sqlx`, `tower-lsp`) — and is skipped rather than read as "no crate may
/// reach it", which would be a different and much louder rule than the file
/// states.
fn declared_wrappers(deny_toml: &str) -> BTreeMap<String, BTreeSet<String>> {
    // `toml::from_str`, not `str::parse` — `impl FromStr for toml::Value` parses
    // a single TOML *value expression*, so a whole document comes back as
    // «unexpected content, expected nothing» at offset 0.
    let doc: toml::Table = toml::from_str(deny_toml).expect("deny.toml is not valid TOML");
    let Some(entries) = doc
        .get("bans")
        .and_then(|b| b.get("deny"))
        .and_then(toml::Value::as_array)
    else {
        return BTreeMap::new();
    };

    let mut out = BTreeMap::new();
    for entry in entries {
        let Some(name) = entry.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let Some(wrappers) = entry.get("wrappers").and_then(toml::Value::as_array) else {
            continue;
        };
        out.insert(
            name.to_string(),
            wrappers
                .iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect(),
        );
    }
    out
}

fn deny_toml() -> String {
    let path = xtask::catalogue::repo_root().join("deny.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The whole situation for one engine, rendered. Printed on every failure,
/// because the person reading it did not run `cargo tree` and the point of not
/// writing the list down here is that the tool is the one that says it.
fn table(engine: &str, declared: &BTreeSet<String>, linkers: &BTreeSet<String>) -> String {
    let mut rows: Vec<String> = Vec::new();
    for name in declared.union(linkers) {
        let d = if declared.contains(name) {
            "declared"
        } else {
            "UNDECLARED"
        };
        let l = if linkers.contains(name) {
            "links it (normal edges)"
        } else {
            "does not link it (dev/build only, or gone)"
        };
        rows.push(format!("  {name} — {d} — {l}"));
    }
    format!(
        "`{engine}` wrappers, declared vs measured:\n{}\n  \
         (authority: `cargo tree -e normal -i {engine} --workspace`)",
        rows.join("\n")
    )
}

// --------------------------------------------------------------------- tests

#[test]
fn no_crate_links_an_engine_without_declaring_it() {
    let meta = depgraph::metadata(None);
    let policy = declared_wrappers(&deny_toml());

    // The guard's own premise. An empty policy means it asserted nothing, which
    // looks exactly like passing.
    assert!(
        !policy.is_empty(),
        "no `[bans].deny` entry in deny.toml carries a `wrappers` list, so this \
         guard checked nothing. Either the engine ring is gone — in which case \
         delete this file and the rule it enforces — or the parse is broken."
    );

    let mut failures = Vec::new();
    for (engine, declared) in &policy {
        let linkers = depgraph::reachers(&meta, engine, Edges::Linking);
        assert!(
            !linkers.is_empty(),
            "no workspace crate reaches `{engine}` over a normal edge at all, yet \
             deny.toml declares wrappers for it. Either the ban outlived the \
             dependency (delete the entry) or the graph walk is broken."
        );
        let undeclared: Vec<&String> = linkers.difference(declared).collect();
        if !undeclared.is_empty() {
            failures.push(format!(
                "`{engine}` is LINKED by {} crate(s) that deny.toml does not name: {}\n{}",
                undeclared.len(),
                undeclared
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                table(engine, declared, &linkers),
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{}\n\n\
         `cargo deny check bans` cannot see these: its `wrappers` list matches \
         DIRECT parents of the banned crate, and every crate above reaches the \
         engine through another one. deny.toml's own comment calls adding a name \
         there «declaring that crate native, and that is the review the rule \
         exists to force» — so the repair is to take that review and add the \
         name with its reason, or to remove the edge.",
        failures.join("\n\n"),
    );
}

/// The other direction: a name in a `wrappers` list that no longer has anything
/// to do with the engine it is declared against.
///
/// `Edges::Any` and not `Edges::Linking`, because a dev-dependency on an engine
/// is a legitimate reason to be in the list — cargo-deny counts a dev parent as
/// a direct parent, and three of `duckdb`'s five entries are exactly that. What
/// this catches is the entry that has become fiction: the crate dropped the
/// dependency entirely, or was renamed, and the list kept the name.
#[test]
fn every_declared_wrapper_still_reaches_its_engine() {
    let meta = depgraph::metadata(None);
    let policy = declared_wrappers(&deny_toml());
    let members = depgraph::member_names(&meta);

    let mut stale = Vec::new();
    for (engine, declared) in &policy {
        let anyone = depgraph::reachers(&meta, engine, Edges::Any);
        for name in declared {
            if !members.contains(name) {
                stale.push(format!(
                    "  `{engine}` names `{name}`, which is not a workspace member \
                     at all — a rename or a deletion left the list behind"
                ));
            } else if !anyone.contains(name) {
                stale.push(format!(
                    "  `{engine}` names `{name}`, which reaches it on no edge of \
                     any kind — the dependency is gone and the declaration is not"
                ));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "deny.toml declares wrappers that are fiction:\n{}\n\n\
         A list that keeps names it no longer needs is how the previous version \
         of this entry came to name four crates and miss the one that linked a \
         database. Delete the line.",
        stale.join("\n"),
    );
}

// ------------------------------------------- the failure modes, each proved
//
// The two tests above are green once the tree is right, which is exactly when a
// guard stops demonstrating that it works. These feed the same pure function
// inputs that should fail, so every branch above is known to fire. The walk's
// own failure modes are proved beside it, in `xtask::depgraph`.

#[cfg(test)]
mod fires {
    use super::*;

    #[test]
    fn only_entries_with_a_wrappers_list_are_a_ring() {
        let policy = declared_wrappers(
            r#"
[bans]
deny = [
    { name = "sqlx", reason = "outright" },
    { name = "duckdb", wrappers = ["a", "b"], reason = "the ring" },
]
"#,
        );
        assert_eq!(policy.len(), 1, "an outright ban is not an engine ring");
        assert_eq!(
            policy["duckdb"],
            ["a".to_string(), "b".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn a_bans_table_that_is_gone_reads_as_empty_rather_than_as_agreement() {
        assert!(
            declared_wrappers("[licenses]\nallow = [\"MIT\"]\n").is_empty(),
            "the anchor assertion in the test above is what turns this into a \
             failure; the parse itself must not invent a policy"
        );
    }
}
