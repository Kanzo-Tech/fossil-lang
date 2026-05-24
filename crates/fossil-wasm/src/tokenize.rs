//! `tokenize` export — single grammar source of truth across compiler, LSP, editor (ADR-0030).
//!
//! ## What it provides
//!
//! - [`tokenize_native`] — pure-Rust `Vec<TokenRow>` mirror. Cargo-test reachable
//!   without a `JS` runtime (`wasm-bindgen` intrinsics panic on native — see
//!   lib.rs header for the `*_native` precedent from 07-02).
//! - [`tokenize`] — `#[wasm_bindgen]` `JS`-facing wrapper. Serialises
//!   `Vec<TokenRow>` to `JsValue` via `serde_wasm_bindgen`. Throws `JsError` on
//!   serialisation failure (extremely rare — the rows are plain numbers).
//! - [`semantic_legend`] — re-exports the Phase-6 06-07 semantic-token legend
//!   over the `wasm-bindgen` boundary so `CodeMirror` (in
//!   `@fossil-lang/codemirror-fossil`) can map LSP `semanticTokens` responses
//!   to highlight categories.
//!
//! ## Token kind stability (the public Rust ↔ `JS` contract)
//!
//! `TokenRow.kind` is `fossil_syntax::lexer::Token as u32`. The numeric value
//! comes from the enum's discriminant — i.e. variant declaration order in
//! `fossil_syntax::lexer::Token`. Per ADR-0030 "Negative consequences":
//!
//! - APPENDING a new variant at the end is backwards-compatible — old
//!   consumers see the new tag as an unknown highlight category (rendered with
//!   default tag, no crash).
//! - REORDERING existing variants is a BREAKING CHANGE for
//!   `@fossil-lang/codemirror-fossil` consumers (the tag table maps numeric kind
//!   → `CodeMirror` tag and is keyed on the discriminant). The tag table lives
//!   in `packages/codemirror-fossil/src/tags.ts` (plan 08-08) and MUST be
//!   updated in lockstep with any reorder.
//!
//! ## Salsa boundary
//!
//! `tokenize` does NOT enter the Salsa graph. It is a plain function over
//! `&str` — no `&dyn Db`, no interning, no tracked query. This is intentional
//! per CONTEXT.md "Architectural invariants": `MAX_PER_MAPPING_FAN_OUT=1`
//! stays at 1; no new Salsa fan-out lands.

use fossil_syntax::lexer::raw_lex;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// One token row in the [`tokenize`] return array.
///
/// `kind` is `fossil_syntax::lexer::Token as u32`. `start` + `end` are byte
/// offsets into the source string (UTF-8 byte indices; the `JS` side converts
/// to UTF-16 code-unit offsets via `LineIndex` if needed — for v0.1 the
/// `CodeMirror` `StreamParser` consumes byte offsets directly because it
/// operates on the same source string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenRow {
    pub kind: u32,
    pub start: u32,
    pub end: u32,
}

/// Native-reachable tokenize.
///
/// Returns a `Vec<TokenRow>` driven by [`fossil_syntax::lexer::raw_lex`].
/// Drops the lexer's `Err` variants (`Logos` rejects unrecognised chars; the
/// parser's recovery layer produces real diagnostics downstream — same
/// contract as `raw_lex` itself).
///
/// Use this from `cargo test -p fossil-wasm --test tokenize`. The
/// `#[wasm_bindgen]` wrapper [`tokenize`] cannot be called from native tests
/// because `serde_wasm_bindgen::to_value` calls `wasm-bindgen` intrinsics
/// that panic on non-wasm32 targets (the same constraint that motivates the
/// `*_rows` / `*_result` split in lib.rs).
#[must_use]
pub fn tokenize_native(text: &str) -> Vec<TokenRow> {
    raw_lex(text)
        .into_iter()
        .map(|(tok, range)| TokenRow {
            kind: tok as u32,
            start: u32::try_from(range.start).expect("source < 4 GiB"),
            end: u32::try_from(range.end).expect("source < 4 GiB"),
        })
        .collect()
}

/// `JS`-facing tokenize. Returns the same row stream as [`tokenize_native`],
/// serialised via `serde_wasm_bindgen`. Consumed by
/// `@fossil-lang/codemirror-fossil`'s `StreamParser` (plan 08-08).
///
/// # Errors
///
/// Returns a JS error only if `serde_wasm_bindgen` fails to serialise the
/// row vector (effectively impossible for `Vec<{u32,u32,u32}>` — kept on the
/// signature for forward compatibility if `TokenRow` ever grows non-trivial
/// fields).
#[wasm_bindgen]
pub fn tokenize(text: &str) -> Result<JsValue, JsError> {
    let rows = tokenize_native(text);
    serde_wasm_bindgen::to_value(&rows).map_err(JsError::from)
}

/// `JS`-facing re-export of the Phase-6 06-07 semantic-token legend.
///
/// Returns `{ tokenTypes: string[], tokenModifiers: string[] }` — the exact
/// LSP `SemanticTokensLegend` shape. `CodeMirror` (plan 08-08 +
/// `packages/codemirror-fossil/src/tags.ts`) uses this to translate
/// LSP `semanticTokens/full` response indices into highlight tag names.
///
/// Delegates to [`fossil_ide::semantic_legend`] (the 06-07 authoritative
/// definition) — NO duplicate legend definition lives here.
///
/// # Errors
///
/// Returns a JS error only if `serde_wasm_bindgen` fails (impossible in
/// practice for the legend shape).
#[wasm_bindgen]
pub fn semantic_legend() -> Result<JsValue, JsError> {
    let legend = fossil_ide::semantic_legend();
    serde_wasm_bindgen::to_value(&legend).map_err(JsError::from)
}
