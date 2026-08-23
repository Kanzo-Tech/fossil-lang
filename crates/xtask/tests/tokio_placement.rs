//! Repo-wide guard: `tokio` never reaches a wasm build, and the rule that says
//! so names no crate.
//!
//! # Why this exists
//!
//! `CLAUDE.md`'s hard rule read **"no `tokio` outside `fossil-lsp`"** from the
//! day it was written until 2026-08-20. It was inverted: `fossil-lsp` has no
//! `tokio` in its manifest and never had, so the rule named as its sole
//! exception the one crate that did not need one, and a contributor obeying it
//! literally would have rejected three correct manifests and approved the crate
//! that had dropped the dependency. The rule was prose, and prose does not go
//! red.
//!
//! It was replaced with a corrected list of three crate names, which is the
//! same failure with a later expiry date. `crates/xtask/src/main.rs` already
//! argues the general case for the WASM gate — the two hand-maintained `-p …`
//! lists that preceded `wasm_closure()` had drifted to 9 crates versus 6 — and
//! the fix there was to derive the set and forbid writing it down. This guard
//! applies the same treatment to `tokio`.
//!
//! # What this proves
//!
//! 1. **No crate in the wasm closure can pull `tokio` into a wasm artefact.**
//!    The closure is re-derived here the way `xtask::wasm_check` derives it —
//!    cdylib crates plus anything declaring `[package.metadata.fossil] wasm =
//!    true`, transitively closed over workspace members — so the two cannot
//!    disagree about what "a wasm build" means. Within it, a `tokio` normal
//!    dependency is a failure unless it is gated by exactly
//!    `cfg(not(target_arch = "wasm32"))`.
//! 2. **The rule in `CLAUDE.md` names no crate.** Every bullet there that
//!    mentions `tokio` is scanned for the name of a workspace member, matched
//!    as a whole word against the member list `cargo metadata` reports. A list
//!    that cannot be written down cannot go stale, which is the only defect
//!    this rule has ever actually had.
//!
//! Both halves read the tree. Nothing here restates a fact that lives
//! somewhere else, so there is no second copy to drift.
//!
//! # What this CANNOT prove
//!
//! - **That `tokio` would fail to compile for `wasm32`.** This guard reasons
//!   about the dependency graph, not the build. `cargo xtask wasm-check` is
//!   still the thing that compiles, and it needs a wasm-capable `clang` that
//!   Apple does not ship; this test needs neither, which is why it can run in
//!   `cargo test` on any machine.
//! - **That a dev-dependency is harmless.** It is excluded because the WASM
//!   gate runs `cargo check --target wasm32-unknown-unknown` *without*
//!   `--all-targets`, so `tests/` is never built for wasm32. Add
//!   `--all-targets` to that gate and this exclusion becomes wrong, silently.
//! - **That a build-dependency is harmless.** Build scripts run on the host, so
//!   one is treated as out of the wasm artefact. That is true of the artefact
//!   and says nothing about whether the build script itself is sane.
//! - **That the justification comments exist.** `CLAUDE.md` also requires each
//!   `tokio` dependency to say why beside itself in its own `Cargo.toml`. Two
//!   of the four in the tree do not, and that clause is still prose.
//! - **That the crate holding `tokio` should hold it.** Nothing here has an
//!   opinion about whether `fossil-mcp` needs a runtime; only about whether a
//!   wasm artefact can reach one.
//!
//! # Why it lives in `xtask`
//!
//! Same reason as `snapshot_hygiene.rs`: `xtask` is the one crate whose subject
//! is the repository rather than a layer of the compiler, and it can read every
//! sibling's manifest without inventing a dependency. It is also nobody's
//! dependency, so this stays green while the compiler is red.
//!
//! There is no new CI step, on purpose. `cargo test --workspace` runs it, and
//! `CONTRIBUTING.md` is explicit that a second gate over an existing `cargo
//! test` is one idea in two places.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The one `cfg` this guard reads as proof that a dependency is outside the
/// wasm build. Anything else is unrecognised — and unrecognised fails, because
/// a guard that guesses at a `cfg` expression it cannot evaluate is worse than
/// one that says it cannot.
const NATIVE_GATE: &str = "cfg(not(target_arch = \"wasm32\"))";

/// How one crate holds `tokio`, derived from the dependency table
/// `cargo metadata` reports for it.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Hold {
    /// A normal dependency behind [`NATIVE_GATE`]. Out of the wasm build.
    NativeGated,
    /// Only ever a dev- or build-dependency. Out of the wasm artefact — see
    /// the two caveats in the header.
    OffArtefact,
    /// A normal dependency that a wasm build would compile. `gate` is the
    /// `cfg` it carried, if it carried one this guard could not read.
    Reaches { gate: Option<String> },
}

impl Hold {
    /// The stronger of two holds, where "stronger" means closer to the wasm
    /// artefact. A crate that names `tokio` twice is described by its worst
    /// naming, not its last one — `fossil-df` has both a gated normal
    /// dependency and an ungated dev one, and it is the normal one that
    /// decides.
    fn worse(self, other: Hold) -> Hold {
        match (&self, &other) {
            (Hold::Reaches { .. }, _) => self,
            (_, Hold::Reaches { .. }) => other,
            (Hold::NativeGated, _) | (_, Hold::NativeGated) => Hold::NativeGated,
            _ => Hold::OffArtefact,
        }
    }

    fn describe(&self) -> String {
        match self {
            Hold::NativeGated => format!("normal, gated `{NATIVE_GATE}`"),
            Hold::OffArtefact => "dev/build only".to_string(),
            Hold::Reaches { gate: None } => "normal, UNGATED".to_string(),
            Hold::Reaches { gate: Some(g) } => format!("normal, gated `{g}` (unreadable)"),
        }
    }
}

/// How `pkg` holds `dep_name`, or `None` if it does not name it at all.
fn hold_of(pkg: &Value, dep_name: &str) -> Option<Hold> {
    let mut found: Option<Hold> = None;
    for d in pkg["dependencies"].as_array()? {
        if d["name"].as_str() != Some(dep_name) {
            continue;
        }
        let target = d["target"].as_str();
        let this = match d["kind"].as_str() {
            // `null` is cargo's spelling of a normal dependency.
            None => match target {
                Some(t) if t == NATIVE_GATE => Hold::NativeGated,
                Some(t) => Hold::Reaches {
                    gate: Some(t.to_string()),
                },
                None => Hold::Reaches { gate: None },
            },
            Some(_) => Hold::OffArtefact,
        };
        found = Some(match found {
            Some(prev) => prev.worse(this),
            None => this,
        });
    }
    found
}

/// Every workspace member that names `dep_name`, with how it holds it.
fn holders(meta: &Value, dep_name: &str) -> BTreeMap<String, Hold> {
    let ws = workspace_ids(meta);
    let mut out = BTreeMap::new();
    for p in meta["packages"].as_array().expect("packages") {
        let id = p["id"].as_str().expect("package id");
        if !ws.contains(id) {
            continue;
        }
        if let Some(h) = hold_of(p, dep_name) {
            out.insert(p["name"].as_str().expect("package name").to_string(), h);
        }
    }
    out
}

fn workspace_ids(meta: &Value) -> HashSet<&str> {
    meta["workspace_members"]
        .as_array()
        .expect("workspace_members")
        .iter()
        .map(|v| v.as_str().expect("member id"))
        .collect()
}

/// The wasm closure, derived the way `xtask::wasm_check` derives it: cdylib
/// crates and crates declaring `[package.metadata.fossil] wasm = true`, closed
/// over workspace-member edges.
///
/// This is a second implementation of `crates/xtask/src/main.rs::wasm_closure`
/// and it is deliberate: that one is a private `fn` in a binary target, so a
/// test cannot call it, and copying its *inputs* (the two roots and the
/// metadata graph) keeps them answering the same question even though neither
/// can call the other. What must not be copied is a crate list, and neither
/// side has one.
fn wasm_closure(meta: &Value) -> BTreeSet<String> {
    let ws = workspace_ids(meta);

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
        let declares_wasm = p["metadata"]["fossil"]["wasm"].as_bool() == Some(true);
        if is_cdylib || declares_wasm {
            roots.push(id);
        }
    }

    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in meta["resolve"]["nodes"].as_array().expect("resolve.nodes") {
        let id = n["id"].as_str().expect("node id");
        if !ws.contains(id) {
            continue;
        }
        adj.insert(
            id,
            n["dependencies"]
                .as_array()
                .expect("node dependencies")
                .iter()
                .filter_map(|d| d.as_str())
                .filter(|d| ws.contains(d))
                .collect(),
        );
    }

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

    seen.iter()
        .filter_map(|id| id_name.get(id).map(|n| (*n).to_string()))
        .collect()
}

// ---------------------------------------------------------------- CLAUDE.md

/// Every markdown bullet in `md` whose text mentions `word`, as
/// `(1-based line of the `- `, the bullet joined into one line)`.
///
/// A bullet runs from its `- ` to the next `- ` at any indent, the next
/// heading, or a blank line — which is how the file is actually written.
fn bullets_mentioning(md: &str, word: &str) -> Vec<(usize, String)> {
    /// A finished bullet is kept only if it is about `word`.
    fn keep(out: &mut Vec<(usize, String)>, done: Option<(usize, String)>, word: &str) {
        if let Some((n, text)) = done
            && text.contains(word)
        {
            out.push((n, text));
        }
    }

    let mut out = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (i, line) in md.lines().enumerate() {
        let t = line.trim_start();
        let starts = t.starts_with("- ") || t.starts_with("* ");
        let ends = t.is_empty() || line.starts_with('#');
        if starts || ends {
            keep(&mut out, current.take(), word);
        }
        if starts {
            current = Some((i + 1, t[2..].to_string()));
        } else if let Some((_, text)) = current.as_mut() {
            // Only reachable when the bullet is still open: an ending line took
            // it above, leaving `None` here.
            text.push(' ');
            text.push_str(t);
        }
    }
    keep(&mut out, current, word);
    out
}

/// `text` with every backticked span that looks like a path or a filename
/// blanked out, so `` `crates/xtask/tests/tokio_placement.rs` `` does not read
/// as a mention of the crate `xtask`. A backticked span with no `/` and no `.`
/// survives, because `` `fossil-df` `` is exactly the thing being forbidden.
fn blank_backticked_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        if inner.contains('/') || inner.contains('.') {
            out.extend(std::iter::repeat_n(' ', inner.len() + 2));
        } else {
            out.push('`');
            out.push_str(inner);
            out.push('`');
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Whether `word` occurs in `hay` bounded by something that is not part of a
/// crate name, so `fossil-df` does not match inside `fossil-df-wasm`.
fn contains_word(hay: &str, word: &str) -> bool {
    let is_part = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut from = 0;
    while let Some(i) = hay[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before_ok = !hay[..start].chars().next_back().is_some_and(is_part);
        let after_ok = !hay[end..].chars().next().is_some_and(is_part);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// The crate names `bullet` writes down, out of `members`.
fn crates_named(bullet: &str, members: &BTreeSet<String>) -> Vec<String> {
    let text = blank_backticked_paths(bullet);
    members
        .iter()
        .filter(|m| contains_word(&text, m))
        .cloned()
        .collect()
}

// ------------------------------------------------------------------ harness

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/xtask is two levels below the repo root")
        .to_path_buf()
}

fn metadata() -> Value {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .current_dir(repo_root())
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed, so this guard cannot see the workspace at all:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("parse cargo metadata json")
}

fn member_names(meta: &Value) -> BTreeSet<String> {
    let ws = workspace_ids(meta);
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|p| ws.contains(p["id"].as_str().expect("package id")))
        .map(|p| p["name"].as_str().expect("package name").to_string())
        .collect()
}

/// The whole `tokio` situation, rendered — printed on every failure, because
/// the person reading it did not run the command and the point of not writing
/// the list down is that the tool is the one that says it.
fn table(holders: &BTreeMap<String, Hold>, closure: &BTreeSet<String>) -> String {
    holders
        .iter()
        .map(|(name, hold)| {
            let where_ = if closure.contains(name) {
                "IN the wasm closure"
            } else {
                "outside the wasm closure"
            };
            format!("  {name} — {} — {where_}", hold.describe())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tokio_never_reaches_a_wasm_build() {
    let meta = metadata();
    let closure = wasm_closure(&meta);
    let holders = holders(&meta, "tokio");

    // The guard's own premises. Either of these failing means it is asserting
    // nothing, which looks exactly like passing.
    assert!(
        !closure.is_empty(),
        "the wasm closure came out empty, so this guard checked nothing. \
         No cdylib crate and no `[package.metadata.fossil] wasm = true` was \
         found — the derivation is broken, not the repository."
    );
    assert!(
        !holders.is_empty(),
        "no workspace crate names `tokio` at all. Either the dependency really \
         is gone from the tree — in which case delete this guard and the rule \
         it enforces — or the metadata scan is broken."
    );

    let offenders: Vec<String> = closure
        .iter()
        .filter_map(|name| match holders.get(name) {
            Some(Hold::Reaches { gate }) => Some(match gate {
                None => format!(
                    "  {name} — names `tokio` as an ungated normal dependency, \
                     and a wasm build of it would compile one"
                ),
                Some(g) => format!(
                    "  {name} — gates `tokio` with `{g}`, which this guard cannot \
                     evaluate. Use exactly `{NATIVE_GATE}`, or move the dependency, \
                     or teach this guard the cfg — do not leave it unreadable"
                ),
            }),
            _ => None,
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "`tokio` reaches a wasm build:\n{}\n\nthe whole picture:\n{}\n\n\
         The wasm closure is {} crate(s): {}",
        offenders.join("\n"),
        table(&holders, &closure),
        closure.len(),
        closure.iter().cloned().collect::<Vec<_>>().join(", ")
    );
}

#[test]
fn the_tokio_rule_names_no_crate() {
    let meta = metadata();
    let members = member_names(&meta);
    let claude = repo_root().join("CLAUDE.md");
    let md = std::fs::read_to_string(&claude).expect("read CLAUDE.md");

    let bullets = bullets_mentioning(&md, "tokio");
    assert!(
        !bullets.is_empty(),
        "no bullet in {} mentions `tokio`, so the rule this guard enforces is \
         gone and the other half of it passes vacuously. Restore the rule or \
         delete the guard.",
        claude.display()
    );

    let mut offences = Vec::new();
    for (line, bullet) in &bullets {
        for named in crates_named(bullet, &members) {
            offences.push(format!("  CLAUDE.md:{line} names `{named}`"));
        }
    }

    let holders = holders(&meta, "tokio");
    let closure = wasm_closure(&meta);
    assert!(
        offences.is_empty(),
        "the `tokio` rule writes crate names down:\n{}\n\n\
         A hand-kept list is the one defect this rule has ever had: it read \
         \"no tokio outside X\" for months, and X was the crate that had none. \
         State the rule; let the tree be the list. It currently reads:\n{}",
        offences.join("\n"),
        table(&holders, &closure)
    );
}

// ------------------------------------------- the failure modes, each proved
//
// The two tests above are green once the tree is right, which is exactly when
// a guard stops demonstrating that it works. These feed the same pure
// functions inputs that should fail, so every branch above is known to fire.

#[cfg(test)]
mod fires {
    use super::*;

    fn dep(name: &str, kind: Value, target: Value) -> Value {
        serde_json::json!({ "name": name, "kind": kind, "target": target })
    }

    fn pkg(deps: Vec<Value>) -> Value {
        serde_json::json!({ "dependencies": deps })
    }

    #[test]
    fn an_ungated_normal_dependency_reaches() {
        let p = pkg(vec![dep("tokio", Value::Null, Value::Null)]);
        assert_eq!(
            hold_of(&p, "tokio"),
            Some(Hold::Reaches { gate: None }),
            "an ungated normal `tokio` must read as reaching the wasm build"
        );
    }

    #[test]
    fn an_unreadable_cfg_reaches() {
        let p = pkg(vec![dep(
            "tokio",
            Value::Null,
            Value::String("cfg(unix)".into()),
        )]);
        assert_eq!(
            hold_of(&p, "tokio"),
            Some(Hold::Reaches {
                gate: Some("cfg(unix)".into())
            }),
            "a cfg this guard cannot evaluate must fail, not be assumed safe"
        );
    }

    #[test]
    fn the_native_gate_is_matched_exactly() {
        let p = pkg(vec![dep(
            "tokio",
            Value::Null,
            Value::String(NATIVE_GATE.into()),
        )]);
        assert_eq!(hold_of(&p, "tokio"), Some(Hold::NativeGated));

        // One character off is not the gate. This is the branch that would let
        // a typo'd cfg through if the comparison were fuzzy.
        let typo = pkg(vec![dep(
            "tokio",
            Value::Null,
            Value::String("cfg(not(target_arch = \"wasm\"))".into()),
        )]);
        assert!(matches!(
            hold_of(&typo, "tokio"),
            Some(Hold::Reaches { gate: Some(_) })
        ));
    }

    #[test]
    fn a_gated_normal_dependency_outranks_an_ungated_dev_one() {
        // `fossil-df`'s actual shape: both, and the normal one decides.
        let p = pkg(vec![
            dep("tokio", Value::String("dev".into()), Value::Null),
            dep("tokio", Value::Null, Value::String(NATIVE_GATE.into())),
        ]);
        assert_eq!(hold_of(&p, "tokio"), Some(Hold::NativeGated));

        // …and an ungated normal one still beats a gated normal one.
        let p = pkg(vec![
            dep("tokio", Value::Null, Value::String(NATIVE_GATE.into())),
            dep("tokio", Value::Null, Value::Null),
        ]);
        assert_eq!(hold_of(&p, "tokio"), Some(Hold::Reaches { gate: None }));
    }

    #[test]
    fn a_crate_that_does_not_name_tokio_is_not_a_holder() {
        let p = pkg(vec![dep("serde", Value::Null, Value::Null)]);
        assert_eq!(hold_of(&p, "tokio"), None);
    }

    #[test]
    fn a_bullet_naming_a_crate_is_caught_backticked_or_bare() {
        let members: BTreeSet<String> = ["fossil-df", "fossil-df-wasm", "fossil-lsp", "xtask"]
            .into_iter()
            .map(String::from)
            .collect();

        assert_eq!(
            crates_named("- tokio lives in `fossil-df` and fossil-lsp.", &members),
            vec!["fossil-df".to_string(), "fossil-lsp".to_string()],
            "both the backticked and the bare name are a written-down list"
        );

        // A path is not a mention of the crate whose directory it passes
        // through, or this guard could never cite its own file.
        assert!(
            crates_named(
                "tokio: see `crates/xtask/tests/tokio_placement.rs` and `Cargo.toml`.",
                &members,
            )
            .is_empty(),
            "a backticked path must not read as naming a crate"
        );

        // `fossil-df` is a prefix of `fossil-df-wasm`; only the whole word counts.
        assert_eq!(
            crates_named("tokio is in `fossil-df-wasm`.", &members),
            vec!["fossil-df-wasm".to_string()],
        );
    }

    #[test]
    fn a_bullet_is_read_to_its_end_and_no_further() {
        let md = "\
# Hard Rules

- **`tokio` never reaches a wasm build.** It is
  gated in the crates that hold it.
- **No `Box<dyn Trait>` inside Salsa queries.** Nothing to do with runtimes.

Some prose mentioning tokio that is not a bullet.
";
        let found = bullets_mentioning(md, "tokio");
        assert_eq!(found.len(), 1, "found: {found:?}");
        assert_eq!(found[0].0, 3, "the bullet starts on line 3");
        assert!(
            found[0].1.contains("gated in the crates that hold it"),
            "the continuation line belongs to the bullet: {:?}",
            found[0].1
        );
        assert!(
            !found[0].1.contains("Salsa"),
            "the next bullet does not: {:?}",
            found[0].1
        );
    }

    #[test]
    fn a_rule_that_vanished_is_not_a_rule_that_passes() {
        assert!(
            bullets_mentioning("# Hard Rules\n\n- Something else entirely.\n", "tokio").is_empty(),
            "the anchor check must notice the rule is gone"
        );
    }
}
