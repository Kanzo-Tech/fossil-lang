//! **The checked-in `problem.gen.ts` is what `problem.schema.json` says.**
//!
//! The third sibling of `catalogue_generated.rs` and `corpus_generated.rs`, and
//! it exists for their reason: the TypeScript union is printed from the schema,
//! so what is left to check is that somebody changed `Problem`, re-blessed the
//! schema and did not regenerate — or edited the generated file by hand. There
//! is no CI step for `cargo xtask problem --check`, for the reason `catalogue`
//! has none: `cargo test` runs this.
//!
//! # What this cannot prove
//!
//! - **That the schema is current.** `crates/fossil-graph-schema/tests/problem_schema.rs`
//!   holds the schema to the enum; this holds the TypeScript to the schema.
//!   Both must be green for the chain to be.
//! - **That `@fossil-lang/types` uses it well.** `FossilError` and its guard
//!   are held by that package's own vitest suite.

use xtask::catalogue::repo_root;
use xtask::problem;

/// The check `cargo xtask problem --check` runs, as a test.
#[test]
fn the_checked_in_file_matches_the_schema() {
    let root = repo_root();
    for (path, want) in problem::generated() {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let have = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{shown} is generated and must exist: {e}"));
        assert_eq!(
            have, want,
            "{shown} is stale — run `cargo xtask problem` and commit the result"
        );
    }
}

/// The guard has to be able to fail: a parse that found nothing would compare
/// two empty unions and pass.
#[test]
fn the_parse_actually_read_the_schema() {
    let codes = problem::read();
    assert!(
        codes.len() >= 30,
        "problem.schema.json parsed to {} codes",
        codes.len()
    );
    let over = codes
        .iter()
        .find(|c| c.code == "run/over-budget")
        .expect("run/over-budget is a code");
    let fields: Vec<(&str, &str, bool)> = over
        .fields
        .iter()
        .map(|(n, t, r)| (n.as_str(), t.as_str(), *r))
        .collect();
    assert_eq!(
        fields,
        [
            ("budget", "number", true),
            ("consumer", "string", true),
            ("requested", "number", true),
            ("reserved", "number", true),
        ]
    );
    assert!(
        codes.iter().all(|c| !c.title.is_empty()),
        "every code has a title"
    );
}
