//! **The checked-in generated files are what `corpus.bnf` says.**
//!
//! The sibling of `catalogue_generated.rs`, one data file along, and it exists
//! for the same reason: under generation, asserting that the Rust names the same
//! columns as the file is an identity — one was printed from the other — so what
//! is left to check is that somebody edited `corpus.bnf` and did not regenerate,
//! or edited a generated file by hand.
//!
//! There is no CI step for `cargo xtask corpus --check`, for the reason
//! `catalogue` has none: `cargo test` runs this, and a second invocation in a
//! workflow would be a second place for the check to be forgotten.
//!
//! # What this cannot prove
//!
//! - **That the roles are right.** That `subject` is an `identity` and not
//!   a second address is a decision, and `corpus.bnf`'s commentary is where it
//!   is argued. This proves the two projections say what the file says.
//! - **That every generated file is listed.** `corpus::generated()` is the list;
//!   a target added to an emitter and forgotten there is invisible here, exactly
//!   as it is to `--check`.
//! - **That a consumer actually reads the table.** A crate that goes back to
//!   writing `"dense_id"` by hand compiles fine. What makes that visible is the
//!   literal count in `corpus.bnf`'s header falling as columns migrate, and a
//!   person reading a diff.

use xtask::catalogue::repo_root;
use xtask::corpus;

/// The check `cargo xtask corpus --check` runs, as a test.
#[test]
fn the_checked_in_files_match_the_corpus_vocabulary() {
    let root = repo_root();
    for (path, want) in corpus::generated() {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let have = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{shown} is generated and must exist: {e}"));
        assert_eq!(
            have, want,
            "{shown} is stale — run `cargo xtask corpus` and commit the result"
        );
    }
}

/// The guard has to be able to fail: a parse that silently found nothing would
/// make every assertion here compare two empty things and pass.
#[test]
fn the_parse_actually_read_the_file() {
    let cols = corpus::read();
    assert!(
        cols.len() >= 4,
        "corpus.bnf parsed to {} columns; the file declares four",
        cols.len()
    );
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"dense_id") && names.contains(&"src"),
        "the parse missed columns it must see: {names:?}"
    );
}
