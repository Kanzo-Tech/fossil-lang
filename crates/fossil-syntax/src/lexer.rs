//! Logos lexer — raw token stream for the full grammar.bnf §LEXICAL LAYER.
//!
//! The output of [`raw_lex`] is fed to the post-lexer INDENT/DEDENT pass
//! in [`crate::indent`] before the parser consumes it.
//!
//! Per RESEARCH.md §"INDENT/DEDENT lexing eats a week" we keep `Whitespace`
//! and `Newline` as real tokens (NOT `#[logos(skip)]`) because the indent
//! pass needs them to measure leading columns.
//!
//! Phase 2 expansion (grammar.bnf lines 53-87, plus the additional keywords
//! on lines 25-27 and the attribute markers on lines 85-87): added one
//! `Token` variant per terminal Phase 1 lacks. Critical ordering rules
//! (logos uses longest-match, with declaration order as tiebreaker for
//! equal-length matches):
//!
//! - Two-char operators come BEFORE their one-char prefixes
//!   (`<=` before `<`, `>=` before `>`, `<<` before `<`, `>>` before `>`,
//!   `==` before `=`, `!=` before `!`, `->` before `-`, `::` before `:`,
//!   `|>` before `|`, `:=` before `:`).
//! - `Float` regex comes BEFORE `Integer` regex (longest-match selects
//!   `Float` for `1.0` because both regexes start with the same digit).
//! - All keyword `#[token]`s come BEFORE the `Ident` regex so the keyword
//!   wins on equal-length matches.
//! - `AtExport` literal comes BEFORE the `AtAttr` regex so `@export` is
//!   classified as the dedicated keyword rather than a generic attribute.
//! - `Partial` (`_`) is declared AFTER `Ident` so `_foo` stays an `Ident`
//!   (longer match) while the bare `_` falls through to `Partial`.

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

    #[token("in")]
    KwIn,

    #[token("use")]
    KwUse,

    #[token("as")]
    KwAs,

    #[token("and")]
    KwAnd,

    #[token("or")]
    KwOr,

    #[token("not")]
    KwNot,

    #[token("iri")]
    KwIri,

    // ───────────────────────────────────────────────────────────────────
    // Attribute markers (grammar.bnf lines 85-87). `@export` literal MUST
    // come before the `AtAttr` regex so logos picks the keyword.
    // ───────────────────────────────────────────────────────────────────
    #[token("@export")]
    AtExport,

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
    // matches win the tiebreaker.
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,

    /// Partial-application placeholder.
    ///
    /// Logos detects an ambiguity between the bare `_` (this variant) and
    /// the `Ident` regex (which also accepts `_` as a valid single-character
    /// identifier). We resolve in favour of `Partial` for the standalone
    /// case via an explicit higher `priority` than the regex's default of 2.
    /// `_foo` still wins as `Ident` via longest-match, since the regex
    /// matches 4 characters and this literal matches only 1.
    #[token("_", priority = 3)]
    Partial,

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

    /// Environment-variable reference `$IDENT` (grammar.bnf line 48).
    #[regex(r"\$[A-Za-z_][A-Za-z0-9_]*")]
    EnvVar,

    // ───────────────────────────────────────────────────────────────────
    // Multi-character operators. Each MUST be declared before its one-char
    // prefix so logos's longest-match rule selects the multi-char form.
    // ───────────────────────────────────────────────────────────────────
    #[token(":=")]
    Define,

    #[token("::")]
    TypeAnnot,

    #[token("==")]
    Eq,

    #[token("!=")]
    Neq,

    #[token("<=")]
    Le,

    #[token(">=")]
    Ge,

    #[token("->")]
    Arrow,

    #[token("|>")]
    Pipe,

    #[token("<<")]
    TripleOpen,

    #[token(">>")]
    TripleClose,

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

    #[token("&")]
    ShapeAnd,
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
    fn lexes_arrow() {
        assert_eq!(just_kinds("->"), vec![Token::Arrow]);
    }

    #[test]
    fn lexes_type_annot() {
        assert_eq!(just_kinds("::"), vec![Token::TypeAnnot]);
    }

    #[test]
    fn lexes_triple_open_and_close() {
        assert_eq!(just_kinds("<<"), vec![Token::TripleOpen]);
        assert_eq!(just_kinds(">>"), vec![Token::TripleClose]);
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
    fn lexes_shape_and() {
        assert_eq!(just_kinds("&"), vec![Token::ShapeAnd]);
    }

    #[test]
    fn lexes_env_var() {
        assert_eq!(just_kinds("$HOME"), vec![Token::EnvVar]);
        assert_eq!(just_kinds("$_under"), vec![Token::EnvVar]);
    }

    #[test]
    fn lexes_at_export_vs_at_attr() {
        assert_eq!(just_kinds("@export"), vec![Token::AtExport]);
        assert_eq!(just_kinds("@dcat"), vec![Token::AtAttr]);
        assert_eq!(just_kinds("@_x"), vec![Token::AtAttr]);
    }

    #[test]
    fn lexes_float_vs_integer() {
        assert_eq!(just_kinds("1.5"), vec![Token::Float]);
        assert_eq!(just_kinds("42"), vec![Token::Integer]);
        assert_eq!(just_kinds("1_000.5_5"), vec![Token::Float]);
    }

    #[test]
    fn lexes_new_keywords() {
        assert_eq!(just_kinds("in"), vec![Token::KwIn]);
        assert_eq!(just_kinds("use"), vec![Token::KwUse]);
        assert_eq!(just_kinds("as"), vec![Token::KwAs]);
        assert_eq!(just_kinds("and"), vec![Token::KwAnd]);
        assert_eq!(just_kinds("or"), vec![Token::KwOr]);
        assert_eq!(just_kinds("not"), vec![Token::KwNot]);
        assert_eq!(just_kinds("iri"), vec![Token::KwIri]);
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
        assert_eq!(just_kinds("<<"), vec![Token::TripleOpen]);
        assert_eq!(just_kinds("<"), vec![Token::Lt]);
        assert_eq!(just_kinds("<="), vec![Token::Le]);
    }

    #[test]
    fn partial_vs_ident_underscore() {
        // standalone `_` → Partial; `_foo` → Ident (longer match).
        assert_eq!(just_kinds("_"), vec![Token::Partial]);
        assert_eq!(just_kinds("_foo"), vec![Token::Ident]);
    }

    #[test]
    fn at_export_keyword_distinct_from_ident_export() {
        // `@export` is the dedicated keyword; bare `export` (no `@`) is an Ident.
        assert_eq!(just_kinds("@export"), vec![Token::AtExport]);
        assert_eq!(just_kinds("export"), vec![Token::Ident]);
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
