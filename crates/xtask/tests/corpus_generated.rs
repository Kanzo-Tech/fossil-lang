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
//! - **That the roles are right.** That `cluster_id` is a `categorical` and not
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
use xtask::corpus::{self, Role, Where};

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
        cols.len() >= 7,
        "corpus.bnf parsed to {} columns; the file declares at least seven",
        cols.len()
    );
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"dense_id") && names.contains(&"src_dense"),
        "the parse missed columns it must see: {names:?}"
    );
}

/// **The two sets that diverged, held apart on purpose.**
///
/// `fossil-graph` hides every column the writer emits from a field listing;
/// `packages/corpus` excludes only the four `PlacedVertex` surfaces as named
/// members. Five against four, and the difference is `cluster_id`. Asserted
/// here so that a column added to `corpus.bnf` without a role makes a decision
/// rather than silently joining both sets or neither.
#[test]
fn the_categorical_is_writer_emitted_and_is_not_a_named_member() {
    let cols = corpus::read();
    let payload: Vec<_> = cols.iter().filter(|c| c.whence == Where::Payload).collect();
    let named: Vec<&str> = payload
        .iter()
        .filter(|c| matches!(c.role, Role::Address | Role::Identity | Role::Coordinate))
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(payload.len(), 5, "every payload column has a role");
    assert_eq!(named, ["dense_id", "subject", "x", "y"]);
    assert!(
        payload
            .iter()
            .any(|c| c.role == Role::Categorical && !named.contains(&c.name.as_str())),
        "the categorical is emitted by the writer and surfaced by no named member",
    );
}

/// Exactly one payload column is also in the manifest's `properties:`, and that
/// asymmetry is what the manifest-versus-bytes difference reduces to.
#[test]
fn one_column_is_declared_and_it_is_the_identity() {
    let cols = corpus::read();
    let declared: Vec<&str> = cols
        .iter()
        .filter(|c| c.declared)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(declared, ["subject"]);
    assert!(
        cols.iter()
            .find(|c| c.name == "subject")
            .is_some_and(|c| c.role == Role::Identity),
        "the declared column is the identity",
    );
}
