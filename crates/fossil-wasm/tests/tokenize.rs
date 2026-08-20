//! Native-side smoke for the `tokenize` export.
//!
//! Drives `tokenize_native` (the pure-Rust mirror) to catch:
//! - `Token` enum reorders that would silently invalidate the public Rust ↔ JS
//!   contract (`TokenRow.kind`)
//! - Grammar drift between `fossil_syntax::lexer::raw_lex` and any future
//!   incremental tokenizer that might want to replace it
//!
//! The JS-side `tokenize` (the `#[wasm_bindgen]` wrapper) is exercised by the
//! node smoke test under `packages/wasm/`. On native
//! targets the wrapper cannot be called: `serde_wasm_bindgen::to_value` calls
//! wasm-bindgen intrinsics that panic on non-wasm32 — exactly the constraint
//! that motivates the `*_native` split.
//!
//! The tests intentionally avoid pinning exact numeric `kind` values. Pinning
//! would couple this test to `Token` variant declaration order and convert
//! the documented "negative consequence" (REORDER is breaking) into a
//! CI failure on every additive change, defeating the additive-friendly
//! contract.

use fossil_wasm::{TokenRow, tokenize_native};

#[test]
fn tokenize_empty_source_returns_empty_vec() {
    assert_eq!(tokenize_native(""), Vec::<TokenRow>::new());
}

/// The fixture was `prefix ex: <https://example.org/>` — two retired forms, and
/// the `<…>` no longer lexes as one token at all, so the test was measuring the
/// tokenizer over bytes the language has no reading for. A source binding
/// exercises the same invariants over a line the lexer actually has a grammar
/// for: IDENT, `:=`, a member call and a string.
#[test]
fn tokenize_source_binding_returns_expected_rows() {
    let src = "users := io.csv(\"u.csv\")\n";
    let rows = tokenize_native(src);

    // Structural invariants only — no exact numeric `kind` pinning.
    assert!(
        !rows.is_empty(),
        "a source binding should produce >0 tokens"
    );

    // The first token starts at byte 0.
    assert_eq!(rows[0].start, 0);

    // Every row is well-formed: start <= end, ends fit inside the source.
    for r in &rows {
        assert!(r.start <= r.end, "row start <= end: {r:?}");
        assert!(
            r.end as usize <= src.len(),
            "row end {} must fit src len {}: {:?}",
            r.end,
            src.len(),
            r
        );
    }

    // Token ranges are monotonically non-decreasing in source order.
    for w in rows.windows(2) {
        assert!(
            w[0].end <= w[1].start,
            "tokens must be non-overlapping + monotonic: {:?} → {:?}",
            w[0],
            w[1]
        );
    }

    // The lexer keeps trivia (Whitespace, Newline) as real tokens (per
    // lexer.rs header — the INDENT/DEDENT pass needs them). So the maximum
    // `end` reaches all the way to the end of the source.
    let max_end = rows.iter().map(|r| r.end).max().expect("non-empty");
    assert_eq!(max_end as usize, src.len(), "trivia tokens reach EOF");
}

#[test]
fn tokenize_handles_unicode_in_comments() {
    // Multi-byte chars inside a comment (Fossil idents are ASCII-only, but
    // `//`-to-EOL comments accept arbitrary UTF-8 — see lexer.rs Comment
    // regex). The invariant is that byte offsets never exceed the source
    // length, even when chars are multi-byte.
    let src = "// comentário ñ\nusers := io.csv(\"u.csv\")\n";
    let rows = tokenize_native(src);

    assert!(
        !rows.is_empty(),
        "source with comment + source binding emits tokens"
    );

    let max_end = rows.iter().map(|r| r.end).max().expect("non-empty");
    assert!(
        max_end as usize <= src.len(),
        "max end {} must not exceed source byte length {}",
        max_end,
        src.len()
    );

    // No row straddles a multi-byte char boundary (each end is a valid char
    // boundary — Rust slicing would panic otherwise).
    for r in &rows {
        assert!(
            src.is_char_boundary(r.start as usize),
            "row start {} must be a char boundary",
            r.start
        );
        assert!(
            src.is_char_boundary(r.end as usize),
            "row end {} must be a char boundary",
            r.end
        );
    }
}

#[test]
fn tokenize_kind_is_stable_within_one_call() {
    // Lexing the same source twice must produce identical rows — the lexer is
    // pure, no hidden state, no nondeterminism.
    let src = "users := io.csv(\"examples/users.csv\")";
    let a = tokenize_native(src);
    let b = tokenize_native(src);
    assert_eq!(a, b, "tokenize_native must be deterministic");
    assert!(!a.is_empty(), "non-empty source emits tokens");
}
