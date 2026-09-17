//! `CLAUDE.md`, read as a rulebook rather than as prose.
//!
//! Two guards hold a bullet in that file against the tree, and they need the
//! same three things out of it: where a bullet starts and stops, which
//! backticked spans are paths rather than crate names, and which workspace
//! members a bullet writes down. `tests/tokio_placement.rs` uses them to prove
//! its rule names NO crate — the set it governs is derivable, so writing one
//! down can only rot. `tests/substrate_reach.rs` uses them the other way round,
//! to read the one crate name its rule must name, because the subject of that
//! rule is a choice and choices cannot be derived from a graph.
//!
//! Neither direction works if the two guards disagree about what a bullet IS,
//! which is why the parser lives here and not in either of them.

use std::collections::BTreeSet;

/// Every markdown bullet in `md` whose text mentions `needle`, as
/// `(1-based line of the `- `, the bullet joined into one line)`.
///
/// A bullet runs from its `- ` to the next `- ` at any indent, the next
/// heading, or a blank line — which is how `CLAUDE.md` is actually written.
///
/// Fenced code blocks are not treated specially, because nothing inside the
/// one fence in that file begins a line with `- `.
pub fn bullets_mentioning(md: &str, needle: &str) -> Vec<(usize, String)> {
    /// A finished bullet is kept only if it is about `needle`.
    fn keep(out: &mut Vec<(usize, String)>, done: Option<(usize, String)>, needle: &str) {
        if let Some((n, text)) = done
            && text.contains(needle)
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
            keep(&mut out, current.take(), needle);
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
    keep(&mut out, current, needle);
    out
}

/// `text` with every backticked span that looks like a path or a filename
/// blanked out, so `` `crates/xtask/tests/tokio_placement.rs` `` does not read
/// as a mention of the crate `xtask`. A backticked span with no `/` and no `.`
/// survives, because `` `fossil-df` `` is exactly the thing being looked for.
///
/// The blanking is length-preserving, so a line number computed over the result
/// still points into the original.
pub fn blank_backticked_paths(text: &str) -> String {
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
pub fn contains_word(hay: &str, word: &str) -> bool {
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
pub fn crates_named(bullet: &str, members: &BTreeSet<String>) -> Vec<String> {
    let text = blank_backticked_paths(bullet);
    members
        .iter()
        .filter(|m| contains_word(&text, m))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> BTreeSet<String> {
        [
            "fossil-df",
            "fossil-df-wasm",
            "fossil-layout",
            "fossil-lsp",
            "xtask",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    #[test]
    fn a_bullet_naming_a_crate_is_caught_backticked_or_bare() {
        assert_eq!(
            crates_named("- tokio lives in `fossil-df` and fossil-lsp.", &members()),
            vec!["fossil-df".to_string(), "fossil-lsp".to_string()],
            "both the backticked and the bare name are a written-down list"
        );

        // A path is not a mention of the crate whose directory it passes
        // through, or a guard could never cite its own file.
        assert!(
            crates_named(
                "tokio: see `crates/xtask/tests/tokio_placement.rs` and `Cargo.toml`.",
                &members(),
            )
            .is_empty(),
            "a backticked path must not read as naming a crate"
        );

        // `fossil-df` is a prefix of `fossil-df-wasm`; only the whole word counts.
        assert_eq!(
            crates_named("tokio is in `fossil-df-wasm`.", &members()),
            vec!["fossil-df-wasm".to_string()],
        );
    }

    #[test]
    fn a_module_path_is_not_the_crate_it_is_spelled_after() {
        // The substrate rule cites `fossil_df::files::batches_to_parquet` as the
        // baseline its dev edge exists for. Underscores are not hyphens, and the
        // rule's subject must not silently acquire a second crate this way.
        assert!(
            crates_named(
                "the bench measures against `fossil_df::files::batches_to_parquet`.",
                &members(),
            )
            .is_empty(),
            "a Rust path spells the crate with underscores and is not the crate name"
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
            "an anchor check must be able to notice the rule is gone"
        );
    }
}
