//! Logos lexer — raw token stream for the full grammar.bnf §LEXICAL LAYER.
//!
//! The output of [`raw_lex`] is fed to the post-lexer INDENT/DEDENT pass
//! in [`crate::indent`] before the parser consumes it.
//!
//! Per RESEARCH.md §"INDENT/DEDENT lexing eats a week" we keep `Whitespace`
//! and `Newline` as real tokens (NOT `#[logos(skip)]`) because the indent
//! pass needs them to measure leading columns.
//!
//! One `Token` variant per terminal of `grammar.bnf` §LEXICAL LAYER that a
//! production actually consumes. Critical ordering rules (logos uses
//! longest-match, with declaration order as tiebreaker for equal-length
//! matches):
//!
//! - Two-char operators come BEFORE their one-char prefixes
//!   (`<=` before `<`, `>=` before `>`, `<<` before `<`, `>>` before `>`,
//!   `==` before `=`, `!=` before `!`, `|>` before `|`, `:=` before `:`).
//! - `Float` regex comes BEFORE `Integer` regex (longest-match selects
//!   `Float` for `1.0` because both regexes start with the same digit).
//! - All keyword `#[token]`s come BEFORE the `Ident` regex so the keyword
//!   wins on equal-length matches.

use logos::Logos;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    // ───────────────────────────────────────────────────────────────────
    // Trivia — kept (NOT skipped) so the indent pass can see them.
    // ───────────────────────────────────────────────────────────────────
    #[regex(r"[ \t]+")]
    Whitespace,

    #[regex(r"\n")]
    Newline,

    // logos 0.16 flags `[^\n]*` as a greedy dot repetition; we explicitly
    // opt in via `allow_greedy = true` because comment-to-EOL is intentionally
    // greedy and benign (linear in line length, not whole input).
    #[regex(r"//[^\n]*", allow_greedy = true)]
    Comment,

    // ───────────────────────────────────────────────────────────────────
    // Keywords (grammar.bnf lines 25-27, plus Phase 1's `prefix` + `from`).
    // Declared BEFORE the `Ident` regex so logos's tiebreaker selects the
    // dedicated keyword on equal-length matches.
    // ───────────────────────────────────────────────────────────────────
    #[token("prefix")]
    KwPrefix,

    #[token("from")]
    KwFrom,

    // `in`, `use` and `as` were keywords here. The named-graph clause and the
    // import went with the forms nothing below the parser read, and a reserved
    // word with no production is a column name the language refuses for free.
    #[token("and")]
    KwAnd,

    #[token("or")]
    KwOr,

    #[token("not")]
    KwNot,

    #[token("iri")]
    KwIri,

    // ───────────────────────────────────────────────────────────────────
    // Attribute marker. The `@` sigil is lexable but no production consumes
    // an `AtAttr` yet — which spelling attributes get is open (ADR-0057,
    // first amendment §2). Keeping the token means a stray `@foo` reaches the
    // parser as one unexpected token instead of being dropped by logos, and
    // a dropped byte is the parser-hang class of bug (`tests/recovery.rs`).
    // ───────────────────────────────────────────────────────────────────
    #[regex(r"@[A-Za-z_][A-Za-z0-9_]*")]
    AtAttr,

    // ───────────────────────────────────────────────────────────────────
    // Numeric literals. `Float` MUST come before `Integer` so logos
    // longest-match selects `Float` for `1.0` (otherwise `1` is `Integer`
    // and `.0` becomes `Dot Integer`).
    // ───────────────────────────────────────────────────────────────────
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*")]
    Float,

    #[regex(r"[0-9][0-9_]*")]
    Integer,

    // Identifiers — declared AFTER all keywords so longer-or-equal keyword
    // matches win the tiebreaker. A bare `_` is an ordinary one-character
    // identifier: there is no partial application, so nothing else claims it.
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,

    // Double-quoted string literal.
    #[regex(r#""([^"\\]|\\.)*""#)]
    String,

    // Backtick-delimited template literal. Interpolation `${...}` is lexed
    // opaquely as a single token; the parser carves it up at use-site.
    #[regex(r"`([^`\\]|\\.)*`")]
    Template,

    // Absolute IRI <https://...>. The angle brackets are part of the token text.
    #[regex(r"<[^>\s]*>")]
    AbsIri,

    // ───────────────────────────────────────────────────────────────────
    // Multi-character operators. Each MUST be declared before its one-char
    // prefix so logos's longest-match rule selects the multi-char form.
    // ───────────────────────────────────────────────────────────────────
    #[token(":=")]
    Define,

    #[token("==")]
    Eq,

    #[token("!=")]
    Neq,

    #[token("<=")]
    Le,

    #[token(">=")]
    Ge,

    #[token("|>")]
    Pipe,

    // ───────────────────────────────────────────────────────────────────
    // Single-character operators / punctuation.
    // ───────────────────────────────────────────────────────────────────
    #[token("=")]
    Assign,

    /// Single colon — emitted as `SHAPE_SEP` by the indent pass. The parser
    /// disambiguates between mapping headers, prefix decls, prefixed names,
    /// and ternary `T_COLON` based on surrounding context (per `grammar.bnf`
    /// §"DISAMBIGUATION RULES").
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

    #[token("<")]
    Lt,

    #[token(">")]
    Gt,

    #[token("+")]
    Plus,

    #[token("-")]
    Minus,

    #[token("*")]
    Star,

    #[token("/")]
    Slash,

    #[token("%")]
    Percent,

    #[token("?")]
    Question,
    // `&` was `ShapeAnd`, the shape intersection's separator. The intersection
    // was lowered by taking the first shape and dropping the rest without a
    // word, so the token claimed a meaning the compiler did not keep.
    //
    // Logos automatically rejects anything not matched; the indent pass
    // drops the `Err` variants from the `spanned()` iterator below.
}

/// Raw lex pass — produces a stream of tokens with byte ranges.
///
/// Errors (unrecognised characters) are silently dropped here; the parser's
/// recovery layer produces real diagnostics from the resulting token stream.
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

    // ─── Phase 1 invariants (preserved verbatim) ──────────────────────

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

    // ─── Phase 2 per-token coverage ───────────────────────────────────

    #[test]
    fn lexes_pipe() {
        assert_eq!(just_kinds("|>"), vec![Token::Pipe]);
    }

    #[test]
    fn arrow_and_double_colon_are_not_tokens() {
        // There are no type annotations on values and no function arrows, so
        // neither spelling is lexed as one token: `->` is `-` `>` and `::`
        // is two colons. Both are parse errors wherever they appear.
        assert_eq!(just_kinds("->"), vec![Token::Minus, Token::Gt]);
        assert_eq!(just_kinds("::"), vec![Token::Colon, Token::Colon]);
    }

    #[test]
    fn lexes_eq_and_neq() {
        assert_eq!(just_kinds("=="), vec![Token::Eq]);
        assert_eq!(just_kinds("!="), vec![Token::Neq]);
    }

    #[test]
    fn lexes_comparison_pairs() {
        assert_eq!(just_kinds("<="), vec![Token::Le]);
        assert_eq!(just_kinds(">="), vec![Token::Ge]);
        assert_eq!(just_kinds("<"), vec![Token::Lt]);
        assert_eq!(just_kinds(">"), vec![Token::Gt]);
    }

    #[test]
    fn lexes_arithmetic_operators() {
        assert_eq!(just_kinds("+"), vec![Token::Plus]);
        assert_eq!(just_kinds("-"), vec![Token::Minus]);
        assert_eq!(just_kinds("*"), vec![Token::Star]);
        assert_eq!(just_kinds("/"), vec![Token::Slash]);
        assert_eq!(just_kinds("%"), vec![Token::Percent]);
    }

    #[test]
    fn lexes_question() {
        assert_eq!(just_kinds("?"), vec![Token::Question]);
    }

    #[test]
    fn ampersand_is_not_a_token() {
        // `&` was the shape intersection's separator and the intersection is
        // gone, so nothing claims the byte and logos rejects it. Pinning the
        // absence is the half of the old `lexes_shape_and` worth keeping: the
        // day something wants `&` back, this test is where it announces itself.
        assert_eq!(just_kinds("&"), vec![]);
    }

    #[test]
    fn lexes_at_attr() {
        assert_eq!(just_kinds("@dcat"), vec![Token::AtAttr]);
        assert_eq!(just_kinds("@_x"), vec![Token::AtAttr]);
        // `@export` is no longer a keyword: it lexes as an ordinary attribute
        // marker, which no production accepts, so it is a parse error.
        assert_eq!(just_kinds("@export"), vec![Token::AtAttr]);
    }

    #[test]
    fn lexes_float_vs_integer() {
        assert_eq!(just_kinds("1.5"), vec![Token::Float]);
        assert_eq!(just_kinds("42"), vec![Token::Integer]);
        assert_eq!(just_kinds("1_000.5_5"), vec![Token::Float]);
    }

    #[test]
    fn lexes_new_keywords() {
        assert_eq!(just_kinds("and"), vec![Token::KwAnd]);
        assert_eq!(just_kinds("or"), vec![Token::KwOr]);
        assert_eq!(just_kinds("not"), vec![Token::KwNot]);
        assert_eq!(just_kinds("iri"), vec![Token::KwIri]);
    }

    #[test]
    fn in_use_and_as_are_ordinary_identifiers() {
        // Three words this lexer used to reserve. The named-graph clause and
        // the import took them, and both forms went — so a table with an `in`
        // column, or a binding called `use`, parses like any other name.
        assert_eq!(just_kinds("in"), vec![Token::Ident]);
        assert_eq!(just_kinds("use"), vec![Token::Ident]);
        assert_eq!(just_kinds("as"), vec![Token::Ident]);
    }

    // ─── Disambiguation / longest-match guards ────────────────────────

    #[test]
    fn lexes_pipe_not_two_chars() {
        // `|>` MUST tokenise as a single Pipe, not e.g. an unknown bit-or
        // followed by Gt.
        assert_eq!(just_kinds("|>"), vec![Token::Pipe]);
    }

    #[test]
    fn lexes_double_le_correctly() {
        // `<<` is no longer a token of its own: the triple term went with the
        // RDF-specific surface, so nothing claims it and it is two `<`.
        assert_eq!(just_kinds("<<"), vec![Token::Lt, Token::Lt]);
        assert_eq!(just_kinds("<"), vec![Token::Lt]);
        assert_eq!(just_kinds("<="), vec![Token::Le]);
    }

    #[test]
    fn underscore_is_an_ordinary_ident() {
        assert_eq!(just_kinds("_"), vec![Token::Ident]);
        assert_eq!(just_kinds("_foo"), vec![Token::Ident]);
    }

    #[test]
    fn three_chained_pipes_lex_as_three_tokens() {
        let kinds = just_kinds("a |> b |> c");
        // Whitespace tokens interleave; assert the non-trivia shape.
        let non_trivia: Vec<_> = kinds
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace))
            .collect();
        assert_eq!(
            non_trivia,
            vec![
                Token::Ident,
                Token::Pipe,
                Token::Ident,
                Token::Pipe,
                Token::Ident,
            ]
        );
    }

    #[test]
    fn precedence_walk_lexes_every_operator() {
        // Covers grammar.bnf precedence walk: `a or b and c == d + e * f`.
        let kinds = just_kinds("a or b and c == d + e * f");
        let non_trivia: Vec<_> = kinds
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace))
            .collect();
        assert_eq!(
            non_trivia,
            vec![
                Token::Ident,
                Token::KwOr,
                Token::Ident,
                Token::KwAnd,
                Token::Ident,
                Token::Eq,
                Token::Ident,
                Token::Plus,
                Token::Ident,
                Token::Star,
                Token::Ident,
            ]
        );
    }
}
