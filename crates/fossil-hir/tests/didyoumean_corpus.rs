//! Did-you-mean corpus — fixture-style coverage of the Damerau-Levenshtein
//! suggestion heuristic over realistic column-name typos.
//!
//! These typo/candidate pairs mirror the column names the diagnostic corpus
//! (plan 03-08) exercises end-to-end. The threshold is `max(2, typo.len()/3)`
//! (see `crates/fossil-hir/src/didyoumean.rs`).

use fossil_hir::did_you_mean;

/// Realistic column set used across the corpus.
const COLUMNS: &[&str] = &["id", "name", "age", "email", "username", "created_at"];

#[test]
fn corpus_transposition_naem_to_name() {
    assert_eq!(did_you_mean("naem", COLUMNS.iter().copied()), Some("name"));
}

#[test]
fn corpus_deletion_usernme_to_username() {
    assert_eq!(
        did_you_mean("usernme", COLUMNS.iter().copied()),
        Some("username")
    );
}

#[test]
fn corpus_substitution_emial_to_email() {
    assert_eq!(
        did_you_mean("emial", COLUMNS.iter().copied()),
        Some("email")
    );
}

#[test]
fn corpus_insertion_naame_to_name() {
    assert_eq!(did_you_mean("naame", COLUMNS.iter().copied()), Some("name"));
}

#[test]
fn corpus_unrelated_long_word_no_match() {
    assert_eq!(
        did_you_mean("completely_unrelated_field", COLUMNS.iter().copied()),
        None
    );
}

#[test]
fn corpus_empty_candidates_returns_none() {
    assert_eq!(did_you_mean("name", std::iter::empty::<&str>()), None);
}

#[test]
fn corpus_single_char_typo_on_short_name() {
    // `ig` ↔ `id`: distance 1, threshold max(2, 2/3) = 2 → matches.
    assert_eq!(did_you_mean("ig", COLUMNS.iter().copied()), Some("id"));
}

#[test]
fn corpus_created_at_typo() {
    // `creatd_at` ↔ `created_at`: one deletion, distance 1.
    assert_eq!(
        did_you_mean("creatd_at", COLUMNS.iter().copied()),
        Some("created_at")
    );
}
