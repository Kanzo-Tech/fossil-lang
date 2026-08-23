//! Repo-wide guard: there is one xsd → `Primitive` table, and it is
//! `fossil_graph_schema::Primitive::from_xsd_iri`.
//!
//! # Why this exists
//!
//! `crates/fossil-graph-schema/src/lib.rs` says, in bold, **"This is the only
//! xsd → `Primitive` table in the tree."** Uniqueness was the whole claim and
//! nothing checked it. It is the kind of claim that is true when written and
//! false three commits later for a good local reason: a decoder that needs one
//! more spelling, in a crate that cannot see this one, adds a three-arm `match`
//! rather than a dependency. The same shape happened here before —
//! `fossil_graph_schema::local_name` exists because three copies of a
//! three-line function had accumulated in the MIR lowering, the ShEx descriptor
//! and the SHACL decoder.
//!
//! A second table does not fail anything. It disagrees at one spelling, in one
//! direction, and the column comes out `String` where the other half of the
//! compiler said `Integer`.
//!
//! # What this proves
//!
//! Both halves are read out of the canonical `match`, so nothing here writes an
//! XSD spelling or a `Primitive` variant down:
//!
//! 1. **The XSD local names** it maps — the string literals on the left of its
//!    arms.
//! 2. **The lattice variants** it maps them to — the identifiers on the right.
//!
//! Then every `.rs` file in the repository is scanned for a `match` arm that
//! pairs one of those names with one of those variants. Exactly one file may
//! have any, and it is the canonical one. A second implementation is reported
//! with the arm it wrote.
//!
//! # What this CANNOT prove
//!
//! - **That the table is right.** It says there is one. Whether
//!   `xsd:unsignedInt` should be `Integer` is a question for the page that
//!   states the lattice.
//! - **That a copy is a `match`.** A second table written as a `HashMap`
//!   literal, a `phf` map, a chain of `if`s, or a `starts_with` cascade pairs
//!   the same names with the same variants and does not go through `=>`. This
//!   catches the shape the tree actually keeps writing, not every possible one.
//! - **Anything outside Rust.** `packages/introspect` carries a DuckDB →
//!   primitive table, which is a different source vocabulary and a different
//!   agreement — `packages/introspect/tests/rust-parity.test.ts` is what holds
//!   that one. If an XSD table ever appears in TypeScript, this guard is blind
//!   to it.
//! - **That the one table is REACHED.** A crate that decodes a datatype IRI by
//!   hand into a `String` fallback, naming no variant, is a lost mapping rather
//!   than a competing one, and reads as clean here.
//!
//! # Why it lives in `xtask`
//!
//! Same reason as `snapshot_hygiene.rs` and `tokio_placement.rs`: `xtask` is
//! the one crate whose subject is the repository rather than a layer of the
//! compiler, it can read every sibling's source without inventing a dependency,
//! and it is nobody's dependency, so this stays green while the compiler is
//! red. `cargo test --workspace` runs it; there is no new CI step, on purpose.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The file that owns the claim, relative to the repository root.
const CANONICAL: &str = "crates/fossil-graph-schema/src/lib.rs";

/// The function whose `match` is the table.
const CANONICAL_FN: &str = "fn from_xsd_iri";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/xtask is two levels below the repo root")
        .to_path_buf()
}

/// Every `*.rs` under `dir`, skipping build output and vendored trees.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // `read_dir` order is the filesystem's; sort, or a failure message names a
    // different file on each machine.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The body of the `match` inside `CANONICAL_FN`, from its `{` to the matching
/// `}`.
///
/// Panics rather than returning nothing when the function is not where it was:
/// a guard that silently finds no table and reports no duplicates of it passes
/// by asserting nothing, which is the failure this file exists to avoid.
fn canonical_match(src: &str) -> &str {
    let at = src.find(CANONICAL_FN).unwrap_or_else(|| {
        panic!(
            "the xsd guard cannot find `{CANONICAL_FN}` in {CANONICAL}; the table \
             was renamed or moved and this guard must be repointed — not deleted"
        )
    });
    let body = &src[at..];
    let open = body
        .find("match ")
        .and_then(|m| body[m..].find('{').map(|b| m + b));
    let Some(open) = open else {
        panic!("`{CANONICAL_FN}` in {CANONICAL} no longer opens with a `match`");
    };
    let mut depth = 0usize;
    for (i, c) in body[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &body[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    panic!("`{CANONICAL_FN}`'s `match` in {CANONICAL} is unbalanced");
}

/// Split Rust source into `match`-arm candidates.
///
/// An arm ends at a `,` and a block at a brace, so splitting on those (plus
/// `;`) leaves each arm whole: alternatives are `|`-separated, never
/// comma-separated. A comma inside a comment or a string splits a chunk that
/// was never going to match anyway.
fn arm_candidates(src: &str) -> impl Iterator<Item = &str> {
    src.split([',', '{', '}', ';'])
        .filter(|chunk| chunk.contains("=>"))
}

/// Every string literal in `chunk`, unescaped only far enough to skip `\"`.
fn string_literals(chunk: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = chunk.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut lit = String::new();
        loop {
            match chars.next() {
                None | Some('"') => break,
                Some('\\') => {
                    if let Some(escaped) = chars.next() {
                        lit.push(escaped);
                    }
                }
                Some(other) => lit.push(other),
            }
        }
        out.push(lit);
    }
    out
}

/// Whether `hay` names `::<variant>` as a whole identifier.
fn names_variant(hay: &str, variant: &str) -> bool {
    let needle = format!("::{variant}");
    let is_part = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = hay[from..].find(&needle) {
        let end = from + i + needle.len();
        if !hay[end..].chars().next().is_some_and(is_part) {
            return true;
        }
        from = from + i + needle.len();
    }
    false
}

/// The XSD local names and the lattice variants the canonical table pairs.
fn canonical_tables(src: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let block = canonical_match(src);
    let mut names = BTreeSet::new();
    let mut variants = BTreeSet::new();
    for chunk in arm_candidates(block) {
        let (lhs, rhs) = chunk.split_once("=>").expect("filtered on `=>`");
        let literals = string_literals(lhs);
        if literals.is_empty() {
            continue;
        }
        // `Self::DateTime` / `Primitive::DateTime` — the identifier after the
        // last `::` on the right.
        let Some(variant) = rhs.rsplit("::").next().map(str::trim) else {
            continue;
        };
        let variant = variant.trim_end_matches(&[',', ')'][..]).trim();
        if variant.is_empty()
            || !variant.starts_with(char::is_uppercase)
            || !variant.chars().all(char::is_alphanumeric)
        {
            continue;
        }
        names.extend(literals);
        variants.insert(variant.to_string());
    }
    (names, variants)
}

/// Every arm in `src` pairing one of `names` with one of `variants`, rendered.
fn tabling_arms(src: &str, names: &BTreeSet<String>, variants: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in arm_candidates(src) {
        let (lhs, rhs) = chunk.split_once("=>").expect("filtered on `=>`");
        if !string_literals(lhs).iter().any(|l| names.contains(l)) {
            continue;
        }
        if !variants.iter().any(|v| names_variant(rhs, v)) {
            continue;
        }
        out.push(format!("{} => {}", lhs.trim(), rhs.trim()));
    }
    out
}

#[test]
fn there_is_one_xsd_to_primitive_table() {
    let root = repo_root();
    let canonical_path = root.join(CANONICAL);
    let canonical_src = std::fs::read_to_string(&canonical_path).unwrap_or_else(|e| {
        panic!(
            "the xsd guard cannot read {}: {e}. If the crate moved, repoint this \
             guard — do not delete it",
            canonical_path.display()
        )
    });

    let (names, variants) = canonical_tables(&canonical_src);
    // The guard's own premises. Either failing means it is asserting nothing,
    // which looks exactly like passing.
    assert!(
        names.len() > 1 && variants.len() > 1,
        "the xsd guard read {} name(s) and {} variant(s) out of `{CANONICAL_FN}`; \
         the `match` changed shape and this guard reads nothing:\n  names: {names:?}\
         \n  variants: {variants:?}",
        names.len(),
        variants.len()
    );

    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    assert!(
        !files.is_empty(),
        "found no `.rs` under {}, so this guard scanned nothing — the walk is \
         broken, not the repository",
        root.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    let mut canonical_arms = 0usize;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let arms = tabling_arms(&src, &names, &variants);
        if arms.is_empty() {
            continue;
        }
        if path == &canonical_path {
            canonical_arms = arms.len();
            continue;
        }
        let rel = path.strip_prefix(&root).unwrap_or(path);
        for arm in arms {
            offenders.push(format!("  {}: {arm}", rel.display()));
        }
    }

    assert!(
        canonical_arms > 0,
        "the scan found no xsd → variant arm in {CANONICAL} itself, which is \
         where the table is. The arm reader and the table reader disagree, so \
         a duplicate elsewhere would also be missed."
    );

    assert!(
        offenders.is_empty(),
        "a second xsd → `Primitive` table:\n{}\n\n\
         {CANONICAL} says it is the only one. Route these through \
         `Primitive::from_xsd_iri`, or — if the claim is what should change — \
         change it there and say what the second table is for. Three copies of \
         `local_name` is how that function came to exist.\n\
         The canonical table maps {} spelling(s) onto {} variant(s).",
        offenders.join("\n"),
        names.len(),
        variants.len(),
    );
}

// ------------------------------------------- the failure modes, each proved
//
// The test above is green while the tree is right, which is exactly when a
// guard stops demonstrating that it works. These feed the same pure functions
// the shapes a second table would take.

#[cfg(test)]
mod fires {
    use super::*;

    fn tables() -> (BTreeSet<String>, BTreeSet<String>) {
        let names = ["integer", "gYear", "anyURI"]
            .into_iter()
            .map(String::from)
            .collect();
        let variants = ["Integer", "GYear", "AnyUri"]
            .into_iter()
            .map(String::from)
            .collect();
        (names, variants)
    }

    #[test]
    fn a_second_match_is_found() {
        let (names, variants) = tables();
        let src = "match local {\n    \"gYear\" => Primitive::GYear,\n    _ => None,\n}";
        assert_eq!(
            tabling_arms(src, &names, &variants),
            vec!["\"gYear\" => Primitive::GYear".to_string()]
        );
    }

    #[test]
    fn an_alternation_is_found_by_any_of_its_spellings() {
        let (names, variants) = tables();
        let src = "\"long\" | \"integer\" => Self::Integer,";
        assert_eq!(tabling_arms(src, &names, &variants).len(), 1);
    }

    #[test]
    fn a_spelling_without_a_variant_is_not_a_table() {
        let (names, variants) = tables();
        // A column named `integer` routed somewhere that is not the lattice.
        let src = "\"integer\" => ColumnKind::Numeric,";
        assert!(tabling_arms(src, &names, &variants).is_empty());
    }

    #[test]
    fn a_variant_without_a_spelling_is_not_a_table() {
        let (names, variants) = tables();
        // Reaching the lattice through the one table is the point, not a
        // violation of it.
        let src = "Some(dt) => Primitive::from_xsd_iri(dt).unwrap_or(Primitive::AnyUri),";
        assert!(tabling_arms(src, &names, &variants).is_empty());
    }

    #[test]
    fn a_longer_variant_name_is_not_a_match() {
        let (names, variants) = tables();
        let src = "\"gYear\" => Wire::GYearMonth,";
        assert!(
            tabling_arms(src, &names, &variants).is_empty(),
            "`GYear` must not match inside `GYearMonth`"
        );
    }

    /// The IRI is not the local name, and the canonical table's inverse
    /// direction (`to_xsd_iri`) writes IRIs. It must not read as a second
    /// forward table.
    #[test]
    fn the_inverse_direction_is_not_a_second_table() {
        let (names, variants) = tables();
        let src = "Self::GYear => \"http://www.w3.org/2001/XMLSchema#gYear\",";
        assert!(tabling_arms(src, &names, &variants).is_empty());
    }

    #[test]
    #[should_panic(expected = "cannot find `fn from_xsd_iri`")]
    fn a_table_that_moved_fails_rather_than_passing_quietly() {
        canonical_match("// the lattice lives elsewhere now\n");
    }
}
