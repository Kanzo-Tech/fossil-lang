//! Logos lexer — raw token stream for the Phase 1 grammar subset.
//!
//! The output of [`raw_lex`] is fed to the post-lexer INDENT/DEDENT pass
//! in [`crate::indent`] before the parser consumes it.
//!
//! Per RESEARCH.md §"INDENT/DEDENT lexing eats a week" we keep `Whitespace`
//! and `Newline` as real tokens (NOT `#[logos(skip)]`) because the indent
//! pass needs them to measure leading columns.

use logos::Logos;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    // Whitespace / trivia — kept (NOT skipped) so the indent pass can see them.
    #[regex(r"[ \t]+")]
    Whitespace,

    #[regex(r"\n")]
    Newline,

    // logos 0.16 flags `[^\n]*` as a greedy dot repetition; we explicitly
    // opt in via `allow_greedy = true` because comment-to-EOL is intentionally
    // greedy and benign (linear in line length, not whole input).
    #[regex(r"//[^\n]*", allow_greedy = true)]
    Comment,

    // Keywords — declared BEFORE the IDENT regex so logos's longest-match-then-priority
    // rule selects them (logos uses ordering as a tiebreaker for equal-length matches).
    #[token("prefix")]
    KwPrefix,

    #[token("from")]
    KwFrom,

    // Identifiers
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,

    // Numeric literals (Phase 1 lexes; the parser does not currently consume).
    #[regex(r"[0-9][0-9_]*")]
    Integer,

    // Double-quoted string literal.
    #[regex(r#""([^"\\]|\\.)*""#)]
    String,

    // Backtick-delimited template literal. Interpolation `${...}` is lexed
    // opaquely as a single token; Phase 2 will add a substring lexer pass.
    #[regex(r"`([^`\\]|\\.)*`")]
    Template,

    // Absolute IRI <https://...>. The angle brackets are part of the token text.
    #[regex(r"<[^>\s]*>")]
    AbsIri,

    // Punctuation
    #[token(":=")]
    Define,

    #[token("=")]
    Assign,

    /// Single colon — emitted as `SHAPE_SEP` by the indent pass. The parser
    /// disambiguates between mapping headers, prefix decls, and prefixed names
    /// based on surrounding context (per `grammar.bnf` §"DISAMBIGUATION RULES").
    #[token(":")]
    Colon,

    #[token(".")]
    Dot,

    #[token(",")]
    Comma,

    #[token("(")]
    LParen,

    #[token(")")]
    RParen,

    #[token("{")]
    LBrace,

    #[token("}")]
    RBrace,
    // Logos automatically rejects anything not matched; the indent pass
    // drops the `Err` variants from the `spanned()` iterator below.
}

/// Raw lex pass — produces a stream of tokens with byte ranges.
///
/// Errors (unrecognised characters) are silently dropped; the post-lexer
/// pass and parser produce real diagnostics in later phases. Phase 1 only
/// needs to handle well-formed input from `examples/hello.fossil`.
#[must_use]
pub fn raw_lex(input: &str) -> Vec<(Token, std::ops::Range<usize>)> {
    Token::lexer(input)
        .spanned()
        .filter_map(|(tok, range)| tok.ok().map(|t| (t, range)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn just_kinds(input: &str) -> Vec<Token> {
        raw_lex(input).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn lexes_prefix_decl() {
        let kinds = just_kinds("prefix ex: <https://example.org/>\n");
        assert_eq!(
            kinds,
            vec![
                Token::KwPrefix,
                Token::Whitespace,
                Token::Ident,
                Token::Colon,
                Token::Whitespace,
                Token::AbsIri,
                Token::Newline,
            ]
        );
    }

    #[test]
    fn lexes_source_def() {
        let kinds = just_kinds(r#"users := io.csv("examples/users.csv")"#);
        assert_eq!(
            kinds,
            vec![
                Token::Ident,
                Token::Whitespace,
                Token::Define,
                Token::Whitespace,
                Token::Ident,
                Token::Dot,
                Token::Ident,
                Token::LParen,
                Token::String,
                Token::RParen,
            ]
        );
    }

    #[test]
    fn lexes_template() {
        // `allow(clippy::literal_string_with_formatting_args)` — the `${...}`
        // here is template-literal interpolation in the Fossil DSL, not a
        // Rust format-string placeholder.
        #[allow(clippy::literal_string_with_formatting_args)]
        let kinds = just_kinds("`${ex:}user/${.id}`");
        assert_eq!(kinds, vec![Token::Template]);
    }

    #[test]
    fn from_keyword_distinct_from_ident() {
        let kinds = just_kinds("from users");
        assert_eq!(kinds, vec![Token::KwFrom, Token::Whitespace, Token::Ident]);
    }
}
