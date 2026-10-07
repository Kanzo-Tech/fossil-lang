//! `tokenize_native`, the rows `tokenize` hands a highlighter.

use fossil_syntax::lexer::Token as TokenKind;
use fossil_wasm::{Token, tokenize_native};

#[test]
fn an_empty_source_has_no_tokens() {
    assert_eq!(tokenize_native(""), Vec::<Token>::new());
}

/// Every row names its token, and the rows tile the source in order.
#[test]
fn a_source_binding_tokenizes_by_name() {
    let rows = tokenize_native("users := io.csv(\"u.csv\")");
    let kinds: Vec<TokenKind> = rows.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            TokenKind::Ident,
            TokenKind::Whitespace,
            TokenKind::Define,
            TokenKind::Whitespace,
            TokenKind::Ident,
            TokenKind::Dot,
            TokenKind::Ident,
            TokenKind::LParen,
            TokenKind::String,
            TokenKind::RParen,
        ]
    );
    assert_eq!(
        serde_json::to_value(rows[2]).expect("serialises"),
        serde_json::json!({ "kind": "Define", "start": 6, "end": 8 })
    );
}

/// Offsets are UTF-16 code units: a comment with an accent and an astral
/// character shifts what follows by its width in units, not in bytes.
#[test]
fn offsets_are_utf16_code_units() {
    let src = "// ñ😀\nusers";
    let rows = tokenize_native(src);
    let ident = rows.last().expect("tokens");
    assert_eq!(ident.kind, TokenKind::Ident);
    let units: Vec<u16> = src.encode_utf16().collect();
    assert_eq!(
        String::from_utf16(&units[ident.start as usize..ident.end as usize]).expect("utf-16"),
        "users"
    );
}
