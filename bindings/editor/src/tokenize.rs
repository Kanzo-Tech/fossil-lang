//! `tokenize` export — the compiler's lexer, for an editor's highlighter.
//!
//! [`tokenize_native`] is the row stream as a `Vec`, reachable from `cargo
//! test`; [`tokenize`] is the same rows across `wasm-bindgen`.
//!
//! A row names its token — `"Comment"`, `"KwFrom"` — because a number would be
//! a discriminant, and a host that wrote one down would be broken silently by
//! any reorder of the enum. Its offsets are UTF-16 code units, the units a
//! JavaScript string is indexed in, like every other range on this surface.
//!
//! `tokenize` does not enter the Salsa graph: it is a plain function over
//! `&str`, because its result is already a pure function of the text the
//! editor just typed.

use fossil_syntax::lexer::{Token as TokenKind, raw_lex};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// One token: its name, and where it is in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Token {
    pub kind: TokenKind,
    /// Inclusive start, in UTF-16 code units.
    pub start: u32,
    /// Exclusive end, in UTF-16 code units.
    pub end: u32,
}

/// The rows [`tokenize`] returns, in source order. A byte the lexer has no
/// reading for is skipped: the parser reports it.
#[must_use]
pub fn tokenize_native(text: &str) -> Vec<Token> {
    let mut units = 0_u32;
    let mut at = 0_usize;
    let mut rows = Vec::new();
    for (kind, range) in raw_lex(text) {
        units += utf16_len(&text[at..range.start]);
        let start = units;
        units += utf16_len(&text[range.clone()]);
        at = range.end;
        rows.push(Token {
            kind,
            start,
            end: units,
        });
    }
    rows
}

fn utf16_len(s: &str) -> u32 {
    u32::try_from(s.encode_utf16().count()).expect("source < 4 GiB")
}

/// The program's tokens, for a highlighter.
///
/// # Errors
///
/// `internal/bug` if the rows do not serialise.
#[wasm_bindgen(unchecked_return_type = "Token[]")]
pub fn tokenize(text: &str) -> Result<JsValue, JsValue> {
    crate::to_value("the tokens", &tokenize_native(text))
}
