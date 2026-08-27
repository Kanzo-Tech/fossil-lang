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
//! - [`token_kinds`] — the legend for [`TokenRow::kind`]: every variant NAME,
//!   indexed by the discriminant a row carries.
//!
//! ## Token kind stability (the public Rust ↔ `JS` contract)
//!
//! `TokenRow.kind` is `fossil_syntax::lexer::Token as u32`, i.e. variant
//! declaration order. A consumer that writes those numbers down is broken by
//! any REORDER of the enum, silently — every kind remaps and nothing fails.
//!
//! **That is not a warning any more, it is a shipped legend.** The old
//! `packages/codemirror-fossil/src/tags.ts` held a hand-copied
//! `enum FossilKind { Whitespace = 0, … }` under a comment saying it «MUST be
//! updated in lockstep with any reorder». Nothing could hold that, and by the
//! time `873cbc0` deleted the package the table was wrong in NINE places —
//! `KwPrefix`, `KwIn`, `KwUse`, `KwAs`, `KwIri`, `Template`, `AbsIri`,
//! `EnvVar` and `Pipe` are all variants the lexer no longer has, and `True`,
//! `False` and `Null` are variants it gained. Every discriminant from 3 up was
//! pointing at the wrong token.
//!
//! So the numbers are not the contract. [`token_kinds`] is: a host reads
//! `token_kinds()[row.kind]` and keys on `"Comment"`, never on `2`. Appending a
//! variant stays backwards-compatible (an index past the end of the legend is
//! `undefined`, which a host styles as plain text), and reordering is now
//! harmless rather than silent.
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

/// Every `fossil_syntax::lexer::Token` variant, in discriminant order.
///
/// Index `i` of this slice is the variant whose `as u32` is `i`. That is the
/// property [`token_kinds`] sells, and `kind_order` in
/// `crates/fossil-wasm/tests/tokenize.rs` is what holds it.
const ALL_TOKENS: &[fossil_syntax::lexer::Token] = {
    use fossil_syntax::lexer::Token as T;
    &[
        T::Whitespace,
        T::Newline,
        T::Comment,
        T::KwFrom,
        T::KwAnd,
        T::KwOr,
        T::KwNot,
        T::AtAttr,
        T::True,
        T::False,
        T::Null,
        T::Float,
        T::Integer,
        T::Ident,
        T::String,
        T::Define,
        T::Eq,
        T::Neq,
        T::Le,
        T::Ge,
        T::Assign,
        T::Colon,
        T::Dot,
        T::Comma,
        T::LParen,
        T::RParen,
        T::LBrace,
        T::RBrace,
        T::Lt,
        T::Gt,
        T::Plus,
        T::Minus,
        T::Star,
        T::Slash,
        T::Percent,
        T::Question,
    ]
};

/// One token variant's name.
///
/// The match is EXHAUSTIVE and that is the whole mechanism: adding, removing or
/// renaming a variant of `fossil_syntax::lexer::Token` fails this crate's build
/// until the name table is updated. The obligation the old
/// `packages/codemirror-fossil/src/tags.ts` carried — «MUST be updated in
/// lockstep with any reorder» — was prose against a file nothing checked, and by
/// the time that file was deleted its table had drifted by nine variants.
#[must_use]
const fn kind_name(t: fossil_syntax::lexer::Token) -> &'static str {
    use fossil_syntax::lexer::Token as T;
    match t {
        T::Whitespace => "Whitespace",
        T::Newline => "Newline",
        T::Comment => "Comment",
        T::KwFrom => "KwFrom",
        T::KwAnd => "KwAnd",
        T::KwOr => "KwOr",
        T::KwNot => "KwNot",
        T::AtAttr => "AtAttr",
        T::True => "True",
        T::False => "False",
        T::Null => "Null",
        T::Float => "Float",
        T::Integer => "Integer",
        T::Ident => "Ident",
        T::String => "String",
        T::Define => "Define",
        T::Eq => "Eq",
        T::Neq => "Neq",
        T::Le => "Le",
        T::Ge => "Ge",
        T::Assign => "Assign",
        T::Colon => "Colon",
        T::Dot => "Dot",
        T::Comma => "Comma",
        T::LParen => "LParen",
        T::RParen => "RParen",
        T::LBrace => "LBrace",
        T::RBrace => "RBrace",
        T::Lt => "Lt",
        T::Gt => "Gt",
        T::Plus => "Plus",
        T::Minus => "Minus",
        T::Star => "Star",
        T::Slash => "Slash",
        T::Percent => "Percent",
        T::Question => "Question",
    }
}

/// Native-reachable kind legend. See [`token_kinds`].
#[must_use]
pub fn token_kinds_native() -> Vec<&'static str> {
    ALL_TOKENS.iter().copied().map(kind_name).collect()
}

/// The name of every [`TokenRow::kind`], indexed BY that kind.
///
/// `token_kinds()[row.kind]` is the variant name — `"Comment"`, `"KwFrom"`,
/// `"String"`. A host maps names to highlight categories and never writes a
/// discriminant down, which is the only version of this contract that survives
/// a reorder of the lexer.
///
/// This is the same move [`semantic_legend`] makes for LSP semantic tokens: the
/// indices are meaningless without the legend, so ship the legend.
///
/// A kind past the end of the array is a variant appended by a newer compiler
/// than the host was built against. That is BACKWARDS-COMPATIBLE by
/// construction: the host sees `undefined` and styles the span as plain text.
///
/// # Errors
///
/// Returns a JS error only if `serde_wasm_bindgen` fails to serialise a
/// `Vec<&str>` (impossible in practice).
#[wasm_bindgen(js_name = tokenKinds)]
pub fn token_kinds() -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(&token_kinds_native()).map_err(JsError::from)
}
