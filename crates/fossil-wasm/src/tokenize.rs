//! `tokenize` export — single grammar source of truth across compiler, LSP, editor.
//!
//! ## What it provides
//!
//! - [`tokenize_native`] — pure-Rust `Vec<TokenRow>` mirror. Cargo-test reachable
//!   without a `JS` runtime (`wasm-bindgen` intrinsics panic on native — see
//!   the lib.rs header for the `*_native` precedent).
//! - [`tokenize`] — `#[wasm_bindgen]` `JS`-facing wrapper. Serialises
//!   `Vec<TokenRow>` to `JsValue` via `serde_wasm_bindgen`. Throws `JsError` on
//!   serialisation failure (extremely rare — the rows are plain numbers).
//! - [`semantic_legend`] — re-exports `fossil-ide`'s semantic-token legend
//!   over the `wasm-bindgen` boundary, so a host can map LSP `semanticTokens`
//!   responses to highlight categories.
//!
//! ## Token kind stability (the public Rust ↔ `JS` contract)
//!
//! `TokenRow.kind` is `fossil_syntax::lexer::Token as u32`. The numeric value
//! comes from the enum's discriminant — i.e. variant declaration order in
//! `fossil_syntax::lexer::Token`. What follows from that:
//!
//! - APPENDING a new variant at the end is backwards-compatible — a consumer
//!   keyed on the discriminant sees the new tag as an unknown category.
//! - REORDERING existing variants silently remaps every kind a consumer has
//!   already written down.
//!
//! **This used to name the consumer, and it was gone.** It said the tag table
//! in `packages/codemirror-fossil/src/tags.ts` «MUST be updated in lockstep
//! with any reorder»; `873cbc0` deleted that package with the rest of the React
//! family, so the obligation named a file that does not exist and no guard could
//! ever have held it. There is no in-repo consumer of these discriminants today.
//! The stability property above is real and worth stating; naming an enforcer
//! that is not there made it read as enforced.
//!
//! ## Salsa boundary
//!
//! `tokenize` does NOT enter the Salsa graph. It is a plain function over
//! `&str` — no `&dyn Db`, no interning, no tracked query. That is deliberate:
//! the compiler holds `MAX_PER_MAPPING_FAN_OUT = 1`, meaning at most one
//! tracked query re-executes per mapping when a file changes, and a tracked
//! query added here would multiply against every mapping in the file for a
//! result that is already a pure function of the text the editor just typed.

use fossil_syntax::lexer::raw_lex;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// One token row in the [`tokenize`] return array.
///
/// `kind` is `fossil_syntax::lexer::Token as u32`. `start` + `end` are byte
/// offsets into the source string (UTF-8 byte indices; a `JS` host converts to
/// UTF-16 code-unit offsets via `LineIndex` if it needs them — a highlighter
/// running over the same source string does not).
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
/// serialised via `serde_wasm_bindgen`.
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

/// `JS`-facing re-export of the semantic-token legend.
///
/// Returns `{ tokenTypes: string[], tokenModifiers: string[] }` — the exact
/// LSP `SemanticTokensLegend` shape, which is what turns the indices in a
/// `semanticTokens/full` response into names a host can style.
///
/// Delegates to [`fossil_ide::semantic_legend`], the one definition — a second
/// legend here would desynchronise the indices the native LSP already emits.
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
