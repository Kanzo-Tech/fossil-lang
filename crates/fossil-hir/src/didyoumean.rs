//! Did-you-mean suggestions via Damerau-Levenshtein.
//!
//! Threshold: `max(2, name_len / 3)`. Inspired by the rustc + clap heuristics
//! A single fixed threshold does not scale: 2 catches `naem ↔ name` and
//! misses a transposition inside a longer name. So it scales with the length of the
//! typo so a short field name (e.g. `id`) does not match an unrelated short
//! name, while a longer name (`username`) tolerates more edits.
//!
//! Used by the bidirectional checker's `lookup_field` (plan 03-05 Task 2) when
//! a `.field` access misses every column declared by the CSVW source row —
//! the suggestion is surfaced in the field-not-found diagnostic (SC#1).

use strsim::damerau_levenshtein;

/// Return the candidate closest (by Damerau-Levenshtein distance) to `typo`,
/// if one is within the `max(2, typo.len() / 3)` threshold.
///
/// Exact matches (distance 0) are excluded — the caller is expected to have
/// already checked for an exact hit and not found one, so `did_you_mean` only
/// proposes *alternatives*.
///
/// Returns `None` when `typo` is empty, the candidate list is empty, or no
/// candidate is close enough.
#[must_use]
pub fn did_you_mean<'a>(
    typo: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    if typo.is_empty() {
        return None;
    }
    let max_dist = std::cmp::max(2, typo.len() / 3);
    candidates
        .into_iter()
        .filter_map(|c| {
            let d = damerau_levenshtein(typo, c);
            // `d > 0` excludes exact matches (the caller already handled those
            // and did not find the field); `d <= max_dist` is the threshold.
            if d > 0 && d <= max_dist {
                Some((d, c))
            } else {
                None
            }
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::did_you_mean;

    #[test]
    fn did_you_mean_close_transposition() {
        // `naem` ↔ `name`: one transposition, DL distance 1.
        assert_eq!(did_you_mean("naem", ["name", "id", "age"]), Some("name"));
    }

    #[test]
    fn did_you_mean_one_char_substitution() {
        // `nmae` ↔ `name`: DL distance 1 (a transposition of `m`/`a` plus...
        // actually a substitution-equivalent close match within threshold).
        assert_eq!(did_you_mean("nmae", ["name"]), Some("name"));
    }

    #[test]
    fn did_you_mean_threshold_scales_with_length() {
        // `usernme` ↔ `username`: DL distance 1, threshold max(2, 7/3) = 2.
        assert_eq!(
            did_you_mean("usernme", ["username", "id"]),
            Some("username")
        );
    }

    #[test]
    fn did_you_mean_no_match_for_unrelated_long_word() {
        assert_eq!(did_you_mean("hello_world_xyz", ["name", "id"]), None);
    }

    #[test]
    fn did_you_mean_empty_typo_returns_none() {
        assert_eq!(did_you_mean("", ["name"]), None);
    }

    #[test]
    fn did_you_mean_no_candidates_returns_none() {
        assert_eq!(did_you_mean("name", std::iter::empty::<&str>()), None);
    }

    #[test]
    fn did_you_mean_excludes_exact_match() {
        // An exact match is distance 0, which is excluded — the caller already
        // looked it up and didn't find the field, so we propose alternatives
        // only. With only the exact name present, there is no alternative.
        assert_eq!(did_you_mean("name", ["name"]), None);
    }

    #[test]
    fn did_you_mean_picks_closest_candidate() {
        // `naem` is distance 1 from `name`, distance ≥2 from `nation`.
        assert_eq!(
            did_you_mean("naem", ["nation", "name", "namespace"]),
            Some("name")
        );
    }
}
