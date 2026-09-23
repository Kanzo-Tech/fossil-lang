//! Guard: the three token-type tables in `src/semantic.rs` stay in lock-step.
//!
//! # Why this exists
//!
//! `mod ty`'s doc comment says the numeric value IS the `tokenType` field of the
//! emitted 5-tuples, so the order there **MUST** stay in lock-step with
//! `LEGEND_TYPES`. That was prose. The only test on it,
//! `legend_has_ten_types_and_no_modifiers`, pins index 0 and index 8 and counts
//! to ten; the other eight can be permuted and every Rust test in the tree stays
//! green while the editor paints a string as a number.
//!
//! There is a **third** table nobody mentioned: `legend_type_name` maps the same
//! indices back to names for the snapshot renderer. Permute it alone and the
//! wire is right while the golden file lies about what it says — which is worse
//! than either half being wrong, because the snapshot is the thing a human reads
//! to review the other two.
//!
//! # What this proves
//!
//! `mod ty` is scraped out of the source text — the constants are `pub(super)`,
//! so their NAMES exist nowhere a test can reach at runtime, and the name is the
//! whole content of the claim. Everything else is called, not read:
//!
//! 1. **The indices are a dense 0..n permutation.** A duplicate or a gap means
//!    two token types share a wire slot, or one is unreachable.
//! 2. **`semantic_legend()` has exactly one entry per constant**, and entry `i`
//!    is the `SemanticTokenType` the constant at `i` is NAMED after.
//! 3. **`legend_type_name(i)` answers with that same name**, for every `i`.
//! 4. **An index past the end is not one of the legend's names**, so a stream
//!    that outgrew the legend renders as something a reviewer can see rather
//!    than as a neighbouring type.
//!
//! Names are compared case- and underscore-insensitively, because the three
//! tables spell one concept in three conventions — `ENUM_MEMBER`, `enumMember`,
//! `enum_member` — and demanding one spelling would fail an honest addition.
//!
//! Nothing here writes a token type down. The list lives in one place and this
//! reads it; a copy of the answer cannot notice the answer changing.
//!
//! # What this CANNOT prove
//!
//! - **That the classifier picks the right type for a token.** This is about
//!   the three tables agreeing on what index 5 MEANS, not about which tokens
//!   get index 5. `tests/semantic_tokens.rs`'s snapshot is that, and it renders
//!   through `legend_type_name` — so this guard is what makes that snapshot
//!   readable as evidence rather than as a second unverified table.
//! - **That the legend is the right legend.** Nothing here has an opinion on
//!   whether Fossil should emit a `namespace` type at all. Two of the ten have
//!   no fixture and cannot get one; that is recorded in
//!   `tests/semantic_tokens.rs` and stays a comment.
//! - **That the client agrees.** The legend is a wire contract with an editor
//!   this repository does not contain.
//! - **That a name-shaped match arm is the whole of `legend_type_name`.** It is
//!   called, not scraped, so this notices a wrong answer; it does not notice a
//!   right answer arrived at by a silly route.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use fossil_ide::{legend_type_name, semantic_legend};

/// The source the constants are scraped from, relative to the crate root.
const SEMANTIC_REL: &str = "src/semantic.rs";

fn semantic_source() -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join(SEMANTIC_REL);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "the legend guard cannot read {}: {e}. If the module moved, repoint \
             this guard — do not delete it",
            path.display()
        )
    })
}

/// `(constant name, index)` for every `const … : u32 = N;` inside `mod ty`.
///
/// Panics rather than returning an empty list when the module is not where it
/// was: a guard that silently finds nothing and passes is the failure this file
/// exists to avoid.
fn ty_constants(src: &str) -> Vec<(String, u32)> {
    let start = src.find("mod ty {").unwrap_or_else(|| {
        panic!(
            "the legend guard cannot find `mod ty {{` in {SEMANTIC_REL}; the module \
             was renamed and this guard must be rewritten with it"
        )
    });
    let body = &src[start..];
    let end = body.find("\n}").unwrap_or_else(|| {
        panic!("`mod ty` in {SEMANTIC_REL} has no closing brace at column 0");
    });
    let out = parse_consts(&body[..end]);
    assert!(
        !out.is_empty(),
        "the legend guard read no constants out of `mod ty` in {SEMANTIC_REL}; \
         the module changed shape and this guard is asserting nothing"
    );
    out
}

/// `NAME` and `N` out of every `const NAME: u32 = N;` line in `body`.
fn parse_consts(body: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub(super) const ") else {
            continue;
        };
        let Some((name, rest)) = rest.split_once(": u32 = ") else {
            continue;
        };
        let Some(value) = rest.strip_suffix(';') else {
            continue;
        };
        let index: u32 = value.trim().parse().unwrap_or_else(|e| {
            panic!("`{name}` in `mod ty` has a non-numeric index `{value}`: {e}")
        });
        out.push((name.trim().to_string(), index));
    }
    out
}

/// One concept, three spellings — `ENUM_MEMBER`, `enumMember`, `enum_member` —
/// reduced to the letters they share.
fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

#[test]
fn the_indices_are_a_dense_permutation() {
    let consts = ty_constants(&semantic_source());
    let mut seen: Vec<Option<&str>> = vec![None; consts.len()];
    for (name, index) in &consts {
        let slot = usize::try_from(*index).expect("index fits a usize");
        assert!(
            slot < consts.len(),
            "`ty::{name}` is {index}, past the end of a {}-entry legend — the \
             emitted tokenType would index nothing",
            consts.len()
        );
        assert!(
            seen[slot].is_none(),
            "`ty::{name}` and `ty::{}` both claim wire slot {index}; one of the \
             two will never be distinguishable by a client",
            seen[slot].unwrap_or("?")
        );
        seen[slot] = Some(name);
    }
}

#[test]
fn every_constant_names_the_legend_entry_it_indexes() {
    let consts = ty_constants(&semantic_source());
    let legend = semantic_legend();

    assert_eq!(
        legend.token_types.len(),
        consts.len(),
        "`mod ty` declares {} constants and the legend carries {} types. Both \
         halves are indices into the same wire array, so the shorter one decides \
         where the mismatch starts: {:?} vs {:?}",
        consts.len(),
        legend.token_types.len(),
        consts.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        legend
            .token_types
            .iter()
            .map(lsp_types::SemanticTokenType::as_str)
            .collect::<Vec<_>>(),
    );

    for (name, index) in &consts {
        let slot = usize::try_from(*index).expect("index fits a usize");
        let entry = legend.token_types[slot].as_str();
        assert_eq!(
            normalise(entry),
            normalise(name),
            "`ty::{name} = {index}` but the legend's slot {index} is `{entry}`. \
             A token this crate classifies as {name} arrives at the editor as \
             {entry}."
        );
    }
}

#[test]
fn the_snapshot_renderer_names_the_same_type_the_wire_carries() {
    let consts = ty_constants(&semantic_source());
    for (name, index) in &consts {
        let rendered = legend_type_name(*index);
        assert_eq!(
            normalise(rendered),
            normalise(name),
            "`legend_type_name({index})` says `{rendered}`, but slot {index} is \
             `ty::{name}`. The semantic-tokens snapshot renders through this \
             function, so a reviewer would sign off on a table that disagrees \
             with the bytes it describes."
        );
    }
}

#[test]
fn an_index_past_the_legend_is_not_a_legend_name() {
    let consts = ty_constants(&semantic_source());
    let n = u32::try_from(consts.len()).expect("a legend of fewer than 4 billion types");
    let fallback = normalise(legend_type_name(n));
    for (name, _) in &consts {
        assert_ne!(
            fallback,
            normalise(name),
            "`legend_type_name({n})` — one past the end — renders as `{name}`, \
             so a token stream that outgrew the legend would read as an ordinary \
             {name} in the snapshot instead of as the bug it is"
        );
    }
}

// ------------------------------------------- the failure modes, each proved
//
// The four tests above are green once the tree is right, which is exactly when
// a guard stops demonstrating that it works. These feed the same pure functions
// inputs that should fail.

#[test]
fn a_permuted_module_is_read_as_permuted() {
    let src = "\
mod ty {
    pub(super) const KEYWORD: u32 = 0;
    pub(super) const NAMESPACE: u32 = 2;
    pub(super) const TYPE: u32 = 1;
}
";
    assert_eq!(
        ty_constants(src),
        vec![
            ("KEYWORD".to_string(), 0),
            ("NAMESPACE".to_string(), 2),
            ("TYPE".to_string(), 1),
        ],
        "the scrape must report the indices as written, not as ordered"
    );
}

#[test]
#[should_panic(expected = "cannot find `mod ty {`")]
fn a_module_that_moved_fails_rather_than_passing_quietly() {
    ty_constants("// the constants live somewhere else now\n");
}

#[test]
#[should_panic(expected = "read no constants")]
fn an_empty_module_fails_rather_than_asserting_nothing() {
    ty_constants("mod ty {\n    // nothing yet\n}\n");
}

#[test]
fn three_spellings_of_one_concept_normalise_together() {
    assert_eq!(normalise("ENUM_MEMBER"), normalise("enumMember"));
    assert_eq!(normalise("ENUM_MEMBER"), normalise("enum_member"));
    assert_ne!(normalise("STRING"), normalise("number"));
}
