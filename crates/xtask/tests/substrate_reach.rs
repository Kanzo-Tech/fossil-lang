//! Repo-wide guard: the batch pass does not link the compiler substrate.
//!
//! # What regressed, and what it cost
//!
//! Three computational regimes share this tree: a Salsa-based incremental
//! compiler, a batch heuristic over Parquet, and a bounded reader. They shared
//! a dependency closure too, and the edge that mixed the first two was one
//! `use`. `fossil-layout`'s pass wrote tiles through the `TileWriter` in
//! `fossil_df::files` — 37 lines of Parquet encoder that `fossil-df` itself
//! never called — and that single import put `datafusion`, `fossil-hir`,
//! `fossil-mir`, `fossil-descriptors-output`, `fossil-base` and `salsa` into
//! the normal closure of a pass that resolves no name, checks no type and holds
//! no database. A compiler front-end and a query engine, compiled to run
//! Louvain over an already-written corpus.
//!
//! `39d0fb8` cut it: `TileWriter` is the leaf crate `fossil-tile-writer` now,
//! over `arrow` and `parquet` alone, and `fossil-df` fell to a dev-dependency
//! for the one thing that still needs it — `examples/compaction_pass.rs`, which
//! measures the tiling against `batches_to_parquet` as a baseline encoder. An
//! example links dev-dependencies, so the bench keeps its baseline and the
//! library links no engine. `cargo tree -p fossil-layout -e normal -i salsa`
//! went from a four-branch tree to *nothing to print*.
//!
//! Nothing held that. It was true on the day it was measured and the only thing
//! standing between the tree and its reintroduction was prose — which is the
//! failure `CLAUDE.md` has documented twice already: the two hand-maintained
//! `-p …` lists for the WASM gate that drifted to 9 crates versus 6, and the
//! `tokio` rule that read "no tokio outside X" for months while X was the one
//! crate that had none.
//!
//! # What this proves
//!
//! **Every crate `CLAUDE.md`'s substrate rule names reaches `salsa` over no
//! normal edge.** Two halves, each read off the tree:
//!
//! - The SUBJECT comes out of `CLAUDE.md` — the bullet that names the crate
//!   `` `salsa` `` in backticks, parsed by `xtask::rulebook`, the same parser
//!   `tests/tokio_placement.rs` uses. One declaration, in the file that states
//!   the rule, and this guard reads it rather than keeping a second copy.
//! - The LINKERS come out of `cargo metadata`, through `xtask::depgraph`, the
//!   same walk `tests/engine_reach.rs` uses for the engine ring. **No crate
//!   list is written down here**, and the whole derived table is printed on any
//!   failure, so a red run is diagnosable without re-deriving it by hand.
//!
//! The asymmetry with the `tokio` rule is deliberate and is the reason the two
//! guards cannot be one. That rule must name NO crate, because the set it
//! governs is derivable and a written-down copy can only rot. This rule must
//! name exactly its subject, because *which* regime is forbidden the substrate
//! is a choice, and a choice is not in the graph. `engine_reach.rs` makes the
//! same distinction, with `deny.toml` as the file that holds the choice.
//!
//! # What this CANNOT prove
//!
//! - **That `salsa` stays out of the wasm payload.** It does not, and nothing
//!   here claims it. `xtask::wasm_closure` walks `resolve.nodes[].dependencies`,
//!   which includes dev edges, and `fossil-wasm` and `fossil-df-wasm` are
//!   cdylib roots that take `fossil-hir` and `fossil-df` directly in any case.
//!   The wasm gate's crate set did not shrink when this edge moved and will not
//!   shrink if it comes back. What is kept clean is the PASS's own normal
//!   closure — what a binary linking only `fossil-layout` has to compile, and
//!   what a reader of that crate has to understand.
//! - **That the subject SHOULD be free of the substrate.** It proves the rule
//!   is enforced, not that it is right. The argument for it is in the bullet
//!   and in `39d0fb8`, and this file has no opinion.
//! - **That the dev edge is harmless.** It is excluded on purpose:
//!   `examples/compaction_pass.rs` links `fossil-df` and therefore `salsa`, and
//!   that is the arrangement the repair chose. `-e normal` is the edge
//!   `deny.toml`, the WASM gate and `apps/docs/content.test.ts` all read, so it
//!   is the one to assert on.
//! - **Anything about a feature-gated edge.** `cargo metadata`'s resolve graph
//!   is taken as given, for the default features and the host target — the same
//!   caveat, in the same conservative direction, that `engine_reach.rs` carries.
//! - **That the rule's subject is the right crate to police.** If the bullet
//!   names a different crate tomorrow, this guard measures that one instead.
//!   The anchor test below is the only thing that stops it naming none at all
//!   and passing vacuously.
//!
//! # Why it lives in `xtask`
//!
//! Same reason as `engine_reach.rs`, `tokio_placement.rs` and
//! `snapshot_hygiene.rs`: `xtask` is the crate whose subject is the repository,
//! it can read every sibling's manifest without inventing a dependency, and it
//! is nobody's dependency, so this stays green while the compiler is red. No
//! new CI step — `cargo test --workspace` runs it, and `CONTRIBUTING.md` is
//! explicit that a second gate over an existing `cargo test` is one idea in two
//! places.

use std::collections::{BTreeMap, BTreeSet};

use xtask::depgraph::{self, Edges};
use xtask::rulebook::{blank_backticked_paths, bullets_mentioning, contains_word, crates_named};

/// The substrate the rule is about. Written once, here, because it is what the
/// guard is named after — everything else about it is derived.
const SUBSTRATE: &str = "salsa";

/// Which `CLAUDE.md` bullets declare a subject for the substrate rule, and the
/// crates each one names: `crate name -> 1-based line of the bullet`.
///
/// A declaring bullet is one that spells the substrate as a CRATE — `` `salsa` ``
/// in backticks — and names at least one workspace member. The backticks are
/// the whole discriminator, and they are how this file already distinguishes a
/// crate from the thing it implements: the two bullets about "Salsa queries"
/// and "Salsa interning" are about the framework, spell it bare and capitalised,
/// and name no crate, so neither is read as a declaration.
fn declared_subjects(md: &str, members: &BTreeSet<String>) -> BTreeMap<String, usize> {
    let marker = format!("`{SUBSTRATE}`");
    let mut out = BTreeMap::new();
    for (line, bullet) in bullets_mentioning(md, &marker) {
        // Blank paths first, so `crates/fossil-df/src/salsa.rs` could never
        // read as the crate, and match the marker as a whole word so a
        // hypothetical `salsa-macros` does not.
        let text = blank_backticked_paths(&bullet);
        if !contains_word(&text, SUBSTRATE) {
            continue;
        }
        for named in crates_named(&bullet, members) {
            out.entry(named).or_insert(line);
        }
    }
    out
}

/// The whole situation, rendered. Printed on every failure, because the person
/// reading it did not run `cargo tree`, and the point of not writing the list
/// down is that the tool is the one that says it.
fn table(subjects: &BTreeMap<String, usize>, linkers: &BTreeSet<String>) -> String {
    let declared: Vec<String> = subjects
        .iter()
        .map(|(name, line)| {
            let verdict = if linkers.contains(name) {
                "LINKS IT"
            } else {
                "clean"
            };
            format!("  {name} — declared by CLAUDE.md:{line} — {verdict}")
        })
        .collect();
    let measured: Vec<String> = linkers
        .iter()
        .map(|name| {
            let mark = if subjects.contains_key(name) {
                " — DECLARED FREE OF IT"
            } else {
                ""
            };
            format!("  {name}{mark}")
        })
        .collect();
    format!(
        "the rule's subjects, as CLAUDE.md declares them:\n{}\n\n\
         every workspace crate that links `{SUBSTRATE}` over a normal edge, \
         as measured ({}):\n{}\n  \
         (authority: `cargo tree -e normal -i {SUBSTRATE} --workspace`)",
        declared.join("\n"),
        linkers.len(),
        measured.join("\n"),
    )
}

fn claude_md() -> (std::path::PathBuf, String) {
    let path = xtask::catalogue::repo_root().join("CLAUDE.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    (path, text)
}

// --------------------------------------------------------------------- tests

#[test]
fn the_batch_pass_does_not_link_the_compiler_substrate() {
    let meta = depgraph::metadata();
    let members = depgraph::member_names(&meta);
    let (_, md) = claude_md();

    let subjects = declared_subjects(&md, &members);
    let linkers = depgraph::reachers(&meta, SUBSTRATE, Edges::Linking);

    // The guard's own premises. Either failing means it asserted nothing, which
    // looks exactly like passing.
    assert!(
        !subjects.is_empty(),
        "no bullet in CLAUDE.md declares a crate free of `{SUBSTRATE}`, so this \
         guard checked nothing. The rule is a bullet that names the crate \
         `{SUBSTRATE}` in backticks and names the crate it governs; restore it, \
         or delete this file with the rule it enforces."
    );
    assert!(
        !linkers.is_empty(),
        "no workspace crate reaches `{SUBSTRATE}` over a normal edge at all. \
         Either the incremental compiler is gone — in which case this guard has \
         nothing left to guard — or the graph walk is broken."
    );

    let offenders: Vec<String> = subjects
        .iter()
        .filter(|(name, _)| linkers.contains(*name))
        .map(|(name, line)| {
            format!(
                "  {name} — CLAUDE.md:{line} declares it free of `{SUBSTRATE}`, and it \
                 reaches it over a NORMAL edge"
            )
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "the compiler substrate is back under a batch pass:\n{}\n\n{}\n\n\
         One `use` did this before: `fossil_df::files::TileWriter`, a Parquet \
         encoder `fossil-df` never called, put `salsa` and `datafusion` into the \
         closure of a pass that resolves no name. `39d0fb8` cut it and this \
         guard is what holds it. The repair is to move the borrowed item below \
         the executor — `fossil-tile-writer` is where the last one went — or to \
         demote the edge to a dev-dependency if only a test or an example needs \
         it. Amending the bullet to drop the crate makes this green and is the \
         one repair that is not one.",
        offenders.join("\n"),
        table(&subjects, &linkers),
    );
}

/// The subject must be a crate that still exists, or the rule governs a name
/// and not a thing.
///
/// `crates_named` already filters to workspace members, so a renamed crate
/// leaves the bullet naming nobody — which is exactly what the anchor above
/// catches, and this test is what says so out loud rather than leaving the two
/// failures indistinguishable.
#[test]
fn the_substrate_rule_names_a_crate_that_exists() {
    let meta = depgraph::metadata();
    let members = depgraph::member_names(&meta);
    let (path, md) = claude_md();

    let marker = format!("`{SUBSTRATE}`");
    let bullets = bullets_mentioning(&md, &marker);
    assert!(
        !bullets.is_empty(),
        "no bullet in {} spells `{SUBSTRATE}` as a crate, so the rule this guard \
         enforces is gone and its other half passes vacuously.",
        path.display()
    );

    let subjects = declared_subjects(&md, &members);
    assert!(
        !subjects.is_empty(),
        "{} has {} bullet(s) about the crate `{SUBSTRATE}` and not one of them \
         names a workspace member. Either the rule lost its subject to a rename \
         — the members are: {} — or the bullet was reworded into prose that \
         governs nothing.",
        path.display(),
        bullets.len(),
        members.iter().cloned().collect::<Vec<_>>().join(", "),
    );
}

// ------------------------------------------- the failure modes, each proved
//
// The two tests above are green once the tree is right, which is exactly when a
// guard stops demonstrating that it works. These feed the same pure function
// inputs that should fail. The graph walk's own failure modes are proved beside
// it in `xtask::depgraph`, and the bullet parser's in `xtask::rulebook`.

#[cfg(test)]
mod fires {
    use super::*;

    fn members() -> BTreeSet<String> {
        [
            "fossil-base",
            "fossil-df",
            "fossil-layout",
            "fossil-tile-writer",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    #[test]
    fn a_bullet_naming_the_crate_declares_its_subject() {
        let md = "\
# Hard Rules

- **The batch pass does not link the compiler substrate.** `fossil-layout` must
  reach `salsa` over no normal edge.
";
        assert_eq!(
            declared_subjects(md, &members()),
            [("fossil-layout".to_string(), 3usize)]
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            "the subject is the crate the bullet names, and the line is where it says it"
        );
    }

    #[test]
    fn prose_about_the_framework_is_not_a_declaration() {
        // Both of CLAUDE.md's existing Salsa bullets, verbatim in shape: bare,
        // capitalised, and about the thing rather than the crate. The second
        // one names a crate, which is what makes the backtick rule load-bearing
        // rather than decorative.
        let md = "\
# Hard Rules

- **No `Box<dyn Trait>` inside Salsa queries.** Salsa interns concrete types.

# Anti-patterns

- Putting compiler logic in `fossil-base` — it is the trait + db substrate, and
  Salsa interning needs concrete types.
";
        assert!(
            declared_subjects(md, &members()).is_empty(),
            "a bullet that spells the framework bare governs nothing, or the \
             anti-patterns section would forbid `fossil-base` its own substrate"
        );
    }

    #[test]
    fn a_path_through_the_substrate_is_not_a_declaration() {
        let md = "- see `crates/fossil-df/src/salsa.rs` for how `fossil-layout` used to do it.\n";
        assert!(
            declared_subjects(md, &members()).is_empty(),
            "a backticked path must not read as naming the crate it passes through"
        );
    }

    #[test]
    fn a_rule_that_lost_its_subject_declares_nobody() {
        let md = "- **The batch pass does not link `salsa`.** No crate is named here.\n";
        assert!(
            declared_subjects(md, &members()).is_empty(),
            "this is the state the anchor test exists to catch: a rule that reads \
             well and governs nothing"
        );
    }

    #[test]
    fn the_table_says_which_side_each_name_came_from() {
        let subjects = [("fossil-layout".to_string(), 42usize)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let linkers = ["fossil-base".to_string(), "fossil-layout".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let rendered = table(&subjects, &linkers);
        assert!(
            rendered.contains("fossil-layout — declared by CLAUDE.md:42 — LINKS IT"),
            "the offender is marked on the declared side: {rendered}"
        );
        assert!(
            rendered.contains("fossil-layout — DECLARED FREE OF IT"),
            "and on the measured side, so the two are readable together: {rendered}"
        );
        assert!(
            rendered.contains("fossil-base\n"),
            "a legitimate linker is listed plain, with no verdict attached: {rendered}"
        );
    }
}
