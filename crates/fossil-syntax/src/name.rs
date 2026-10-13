//! A member's NAME, and the one spelling each name has.
//!
//! A column takes its name from the data, and data does not ask the grammar
//! first: LDBC's relationship files head their columns `Person.id|Tag.id`, a
//! spreadsheet export writes `Order Date`, and a header can be a reserved word.
//! `Row.<name>` cannot spell any of them, so a member may also be written as a
//! STRING — `Knows."Person.id"`, the SQL-standard delimited identifier, which is
//! also how `DataFusion` and `DuckDB` spell it and how `jq` writes `."a.b"`.
//! `grammar.bnf, PostfixOp` has the argument and the rejected alternatives.
//!
//! **Each name has exactly one spelling**, and this module is the only place
//! that decides it: bare when the name lexes as one `IDENT`, quoted otherwise.
//! The parser refuses a quoted name that did not need quoting, and every reader
//! that prints a name back to the author — a suggestion, a completion, a hover —
//! asks [`spell`], so the form a diagnostic proposes is the form the parser
//! accepts.

use std::borrow::Cow;

use smol_str::SmolStr;

use crate::kind::{SyntaxKind, SyntaxNode, SyntaxToken};
use crate::lexer::{Token, raw_lex_lossless};

/// Can `name` be written bare? True when the lexer reads it as exactly one
/// `IDENT` — so not a keyword, not `true`/`false`/`null`, and nothing with a
/// byte an identifier cannot hold. Asking the lexer rather than restating its
/// regex is what keeps the two from drifting.
#[must_use]
pub fn is_bare(name: &str) -> bool {
    matches!(
        raw_lex_lossless(name).as_slice(),
        [(Some(Token::Ident), range)] if range.end == name.len()
    )
}

/// The binding name a file becomes in a program — `people` for `people.csv`, as a
/// source a program binds by it: `people := io.csv("people.csv")`.
///
/// The extension of the last segment goes; every character an identifier
/// cannot hold becomes `_`, `/` among them, so `crm/people.csv` is `crm_people`;
/// and a name that still does not lex as one `IDENT` — one that starts with a
/// digit, an empty one, a reserved word such as `from` — is prefixed with `_`.
/// Which characters an identifier holds is ASKED of the lexer, a character at a
/// time, never restated: the answer is whatever `grammar.bnf, IDENT` lexes, and
/// the result always [`is_bare`].
#[must_use]
pub fn binding_name(file: &str) -> String {
    let segment_start = file.rfind('/').map_or(0, |i| i + 1);
    let stem = file[segment_start..]
        .rfind('.')
        .map_or(file, |dot| &file[..segment_start + dot]);
    let held: String = stem
        .chars()
        .map(|c| {
            let mut probe = String::from("a");
            probe.push(c);
            if is_bare(&probe) { c } else { '_' }
        })
        .collect();
    if is_bare(&held) {
        held
    } else {
        format!("_{held}")
    }
}

/// The one spelling of a member called `name`: the name itself when it
/// [`is_bare`], else a STRING whose value is the name — `"`, `\` and `{`
/// escaped as a string literal escapes them, and a line break as `\n`.
#[must_use]
pub fn spell(name: &str) -> Cow<'_, str> {
    if is_bare(name) {
        return Cow::Borrowed(name);
    }
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("{{"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    Cow::Owned(out)
}

/// The value of a STRING token with no hole, its escapes resolved
/// (grammar.bnf, `ESCAPE` and `{{`). The inverse of [`spell`] for a quoted name.
#[must_use]
pub fn string_value(raw: &str) -> String {
    let inner = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(raw);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            c => out.push(c),
        }
    }
    out
}

/// The token that names the member of a member access — `name` in `User.name`,
/// `"Person.id"` in `Knows."Person.id"`. `None` for a call node and for any node
/// that is not a member access.
#[must_use]
pub fn member_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    if node.kind() != SyntaxKind::POSTFIX_EXPR {
        return None;
    }
    let mut toks = node
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        });
    match (toks.next(), toks.next(), toks.next()) {
        (Some(dot), Some(name), None)
            if dot.kind() == SyntaxKind::DOT
                && matches!(name.kind(), SyntaxKind::IDENT | SyntaxKind::STRING) =>
        {
            Some(name)
        }
        _ => None,
    }
}

/// The name a member token stands for: an `IDENT`'s text, or a quoted name's
/// value.
#[must_use]
pub fn token_name(token: &SyntaxToken) -> SmolStr {
    if token.kind() == SyntaxKind::STRING {
        SmolStr::from(string_value(token.text()))
    } else {
        SmolStr::from(token.text())
    }
}

/// [`member_token`] read through [`token_name`].
#[must_use]
pub fn member_name(node: &SyntaxNode) -> Option<SmolStr> {
    member_token(node).map(|t| token_name(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_is_spelled_bare_and_anything_else_is_quoted() {
        assert_eq!(spell("name"), "name");
        assert_eq!(spell("_x1"), "_x1");
        assert_eq!(spell("Person.id"), "\"Person.id\"");
        assert_eq!(spell("Order Date"), "\"Order Date\"");
        assert_eq!(spell("1st"), "\"1st\"");
        assert_eq!(spell(""), "\"\"");
    }

    #[test]
    fn a_reserved_word_is_quoted_and_a_contextual_one_is_not() {
        for word in ["from", "and", "or", "not", "true", "false", "null"] {
            assert_eq!(spell(word), format!("\"{word}\""), "`{word}` is reserved");
        }
        // `type` and `as` are contextual (grammar.bnf, § DISAMBIGUATION RULES):
        // a column may be called either and still be written bare.
        assert_eq!(spell("type"), "type");
        assert_eq!(spell("as"), "as");
        assert_eq!(spell("truestory"), "truestory");
    }

    #[test]
    fn a_file_becomes_a_name_that_lexes_as_one_ident() {
        for (file, name) in [
            ("people.csv", "people"),
            ("Order Date.csv", "Order_Date"),
            ("2024-sales.parquet", "_2024_sales"),
            ("crm/people.v2.csv", "crm_people_v2"),
            ("a.b/c", "a_b_c"),
            ("café.json", "caf_"),
            ("from.csv", "_from"),
            ("true.csv", "_true"),
            ("type.csv", "type"),
            (".csv", "_"),
            ("", "_"),
        ] {
            let got = binding_name(file);
            assert_eq!(got, name, "{file}");
            assert!(is_bare(&got), "`{got}` from `{file}` lexes as one IDENT");
        }
    }

    #[test]
    fn spelling_and_reading_round_trip() {
        for name in [
            "Person.id",
            "a \"b\" c",
            "back\\slash",
            "{brace}",
            "x\ty",
            "café",
        ] {
            assert_eq!(string_value(&spell(name)), name, "{name}");
        }
    }
}
