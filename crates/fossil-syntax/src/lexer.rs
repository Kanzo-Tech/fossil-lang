//! Logos lexer — raw token stream for the whole lexical layer
//! (grammar.bnf, § LEXICAL LAYER).
//!
//! The output of [`raw_lex`] is fed to the post-lexer INDENT/DEDENT pass
//! in [`crate::indent`] before the parser consumes it.
//!
//! Per RESEARCH.md §"INDENT/DEDENT lexing eats a week" we keep `Whitespace`
//! and `Newline` as real tokens (NOT `#[logos(skip)]`) because the indent
//! pass needs them to measure leading columns.
//!
//! One `Token` variant per terminal of the lexical layer
//! (grammar.bnf, § LEXICAL LAYER), and now with no exception — `BOOL` was the
//! last one missing and `True` / `False` close it.
//! `KwPrefix`, `Template`, `AbsIri` and `Pipe` were the four
//! that outlived their surface and all four are gone — with them the backtick,
//! `${`, `<…>` and `|>` stop being tokens at all, so a byte that used to open a
//! retired spelling now reaches the parser as itself.
//!
//! Critical ordering rules (logos uses longest-match, with declaration order as
//! tiebreaker for equal-length matches):
//!
//! - Two-char operators come BEFORE their one-char prefixes
//!   (`<=` before `<`, `>=` before `>`, `==` before `=`, `!=` before `!`,
//!   `:=` before `:`). There is no `<<` and no `>>`, and no `|>`: the
//!   triple term went with the RDF-specific surface, and `<` and `>` have one
//!   reading each (`lexes_double_le_correctly` pins it). With `AbsIri` gone,
//!   `<` no longer even opens: nothing else claims the byte.
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
    // Keywords. The grammar reserves exactly four (grammar.bnf, KEYWORD) —
    // `from`, `and`, `or`, `not`. `true` and `false` are reserved by the same
    // list but are LITERALS rather than keywords, and have their own rules
    // below. Declared BEFORE the `Ident` regex so logos's tiebreaker selects
    // the dedicated keyword on equal-length matches.
    // ───────────────────────────────────────────────────────────────────
    // `prefix` was a keyword here. It introduced the CURIE and the grammar
    // retired both: a program writes full IRIs inside strings and bare names
    // everywhere else, so there is no vocabulary declaration left to open.
    // `prefix` is an ordinary identifier (grammar.bnf, § RESERVED KEYWORDS), and
    // `items::parse_program` recognises the retired LINE by shape — `prefix
    // IDENT :` — so the diagnostic can name what to write instead.
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

    // `iri` was a keyword here. It named the argument of `@subject(iri = …)`,
    // and the identity is an ASSIGNMENT now — so the word named an
    // argument no production takes, and holding a plausible column name hostage
    // for it bought nothing. `iri` is an ordinary identifier, which in a
    // language whose corpus is RDF is the point.

    // ───────────────────────────────────────────────────────────────────
    // Attribute marker — `AT_ATTR := '@' IDENT`, and the sigil is decided.
    // The grammar accepts exactly two names, told apart by
    // position: `@rename` above a type binding, `@subject` as the first line
    // of a mapping body. This lexer emits one token for any name; both the
    // position check and `@rename` itself are the parser's outstanding work.
    // ───────────────────────────────────────────────────────────────────
    #[regex(r"@[A-Za-z_][A-Za-z0-9_]*")]
    AtAttr,

    // ───────────────────────────────────────────────────────────────────
    // Boolean literals — `BOOL := 'true' | 'false'` (grammar.bnf, BOOL).
    //
    // They are LITERALS, not keywords, and that is the whole reason they are
    // tokens: a program writes `verified = true` and there is no binding for
    // the name to resolve against, so leaving them as `Ident` makes a literal
    // look like an unresolved reference. It did, measurably — `expressions`
    // reported `unknown column \`true\`` until this rule existed.
    //
    // Declared BEFORE the `Ident` regex so logos's equal-length tiebreaker
    // selects these, for the same reason the four keywords above are.
    // ───────────────────────────────────────────────────────────────────
    #[token("true")]
    True,

    #[token("false")]
    False,

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

    // Double-quoted string literal. The ONE string spelling, carved into the
    // interpolation run by `crate::indent::carve_interpolations` when it has a
    // hole.
    //
    // There was a `Template` here — the backtick literal and its `${` hole,
    // a second spelling of this one differing only in the delimiter and in `${`
    // for the hole. `"…{expr}…"` is the spelling; a backtick is no longer a
    // token, so logos rejects the byte and the parser names it.
    #[regex(r#""([^"\\]|\\.)*""#)]
    String,

    // There was an `AbsIri` here — `<https://…>`, angle brackets included.
    // Its two consumers, the vocabulary declaration and the property key, are
    // bare names now, and a constant IRI is written as a STRING. The lexical win
    // is that `<` and `>` have one reading each and this lexer no longer has to
    // guess between a comparison and the start of an IRI.

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

    // There is no `Pipe`. `a |> f()` was a second
    // spelling of `a.f()`, and ruling 7 of 2026-08-11 retired it: with members
    // resolved by the type of the receiver, the pipeline had nothing left that
    // the dot could not do. `|` now matches no rule, so it reaches the parser as
    // an ERROR token carrying its text — which is what `parse_expression` pairs
    // with the `>` after it to refuse the form by name.

    // ───────────────────────────────────────────────────────────────────
    // Single-character operators / punctuation.
    // ───────────────────────────────────────────────────────────────────
    #[token("=")]
    Assign,

    /// Single colon — emitted as `SHAPE_SEP` by the indent pass.
    ///
    /// The grammar gives it TWO readings, and disambiguation rule 3 is now the
    /// whole of the rule: the mapping header's `Name : Shape`, and the ternary's
    /// `cond ? a : b`. Which one it is comes from the node, never from the
    /// lexeme — there is no `T_COLON`, and there never was a token for it.
    ///
    /// The third reading — the CURIE's `ex:name` — went with the CURIE, and so
    /// did the no-whitespace-before-the-colon check that existed only to keep
    /// `a : b` out of a ternary (grammar.bnf, § DISAMBIGUATION RULES).
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
    // Logos rejects any byte no rule above matches. Those bytes are NOT
    // dropped — see `raw_lex_lossless`.
}

/// Raw lex pass — every byte of `input` accounted for.
///
/// `None` is logos's «no rule matched here», and its range is kept rather than
/// discarded. [`crate::indent::lex_with_indents`] turns each one into a
/// `SyntaxKind::ERROR` token carrying the offending text, which is what lets
/// `parse("#")` report the character. Before this existed, `#` vanished between
/// the lexer and the parser: the token stream came back empty, `parse_program`
/// broke on `None` immediately, and the LSP's Problems panel showed nothing at
/// all for a file the user could see was wrong.
#[must_use]
pub fn raw_lex_lossless(input: &str) -> Vec<(Option<Token>, std::ops::Range<usize>)> {
    Token::lexer(input)
        .spanned()
        .map(|(tok, range)| (tok.ok(), range))
        .collect()
}

/// Raw lex pass with the unlexable bytes DROPPED.
///
/// The lossy one, and it is lossy on purpose: `fossil-wasm`'s `tokenize` feeds
/// a `CodeMirror` `StreamParser` that has no row shape for a byte with no token
/// kind. Everything inside this crate wants [`raw_lex_lossless`], because a
/// dropped byte is a diagnostic nobody can emit.
#[must_use]
pub fn raw_lex(input: &str) -> Vec<(Token, std::ops::Range<usize>)> {
    raw_lex_lossless(input)
        .into_iter()
        .filter_map(|(tok, range)| tok.map(|t| (t, range)))
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
    fn the_prefix_declaration_is_no_longer_one_token_run() {
        // `prefix ex: <https://example.org/>` used to lex as
        // `KwPrefix Ident Colon AbsIri`. Three of those four tokens are gone:
        // `prefix` is an ordinary identifier (grammar.bnf, § RESERVED KEYWORDS)
        // and the angle brackets are a comparison and its operands.
        // Nothing here says "vocabulary declaration" any more, which is what
        // makes `items::parse_program`'s shape check the only thing that can
        // recognise the retired line and name what replaces it.
        let kinds = just_kinds("prefix ex: <https://example.org/>\n");
        let non_trivia: Vec<_> = kinds
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace | Token::Newline))
            .collect();
        assert_eq!(
            non_trivia,
            vec![
                Token::Ident, // `prefix`
                Token::Ident, // `ex`
                Token::Colon,
                Token::Lt,
                Token::Ident, // `https`
                Token::Colon,
                // AND THE REST OF THE LINE IS A COMMENT. `//` in the scheme is
                // the comment opener, and with `AbsIri` gone nothing claims it
                // first — so `//example.org/>` is trivia and the `>` never
                // arrives as a token at all.
                //
                // The lexical win of dropping `ABS_IRI` is that `<` and `>` get
                // one reading each. Nothing wrote THIS down, and it is the
                // consequence that costs: EVERY absolute IRI in the old corpus
                // comments out the rest of its own line.
                // It is why `items::parse_property_lhs` refuses the
                // form over the whole LINE rather than through its `>` — there
                // is no `>` left to stop at, and nothing after the `//` to save.
                Token::Comment,
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
    fn a_backtick_is_not_a_token() {
        // The backtick literal was a second spelling of the quoted string and
        // died with the CURIE-with-holes it carried.
        // Nothing claims the byte, so logos rejects it — and the lossless pass
        // keeps the range, which is what lets the parser say which character it
        // was rather than dropping it.
        assert_eq!(just_kinds("`"), vec![]);
        assert_eq!(raw_lex_lossless("`"), vec![(None, 0..1)]);
        // `$` went with `${`: it was never a token on its own either.
        assert_eq!(raw_lex_lossless("$"), vec![(None, 0..1)]);
    }

    #[test]
    fn an_absolute_iri_is_not_a_token() {
        // `<https://example.org/>` was ONE `AbsIri`. It is now a comparison
        // operator and its operands, which is the whole lexical win of dropping
        // `ABS_IRI`: `<` has one reading.
        let kinds = just_kinds("<a>");
        assert_eq!(kinds, vec![Token::Lt, Token::Ident, Token::Gt]);
    }

    #[test]
    fn from_keyword_distinct_from_ident() {
        let kinds = just_kinds("from users");
        assert_eq!(kinds, vec![Token::KwFrom, Token::Whitespace, Token::Ident]);
    }

    // ─── Phase 2 per-token coverage ───────────────────────────────────

    #[test]
    fn pipe_is_not_a_token() {
        // `|` matches no rule, so logos yields `None` for it and `>` lexes as
        // `Gt`. The parser pairs the two to refuse `|>` by name.
        assert_eq!(just_kinds("|>"), vec![Token::Gt]);
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
        // Rejected, but not lost: the lossless pass keeps the range so the
        // parser can say which character it was.
        assert_eq!(raw_lex_lossless("&"), vec![(None, 0..1)]);
    }

    #[test]
    fn unlexable_bytes_keep_their_ranges() {
        // One `None` per rejected run, with the range that names the bytes.
        // `raw_lex` drops exactly these and nothing else.
        assert_eq!(raw_lex_lossless("#"), vec![(None, 0..1)]);
        assert_eq!(raw_lex_lossless("##"), vec![(None, 0..1), (None, 1..2)]);
        assert_eq!(
            raw_lex_lossless("a#b"),
            vec![
                (Some(Token::Ident), 0..1),
                (None, 1..2),
                (Some(Token::Ident), 2..3),
            ]
        );
        assert_eq!(just_kinds("a#b"), vec![Token::Ident, Token::Ident]);
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
    }

    #[test]
    fn true_and_false_are_literals_and_not_identifiers() {
        // grammar.bnf, BOOL, whose comment names the exact failure this rule
        // prevents: as `Ident`, `verified = true` resolves `true` against
        // the source row and reports `unknown column \`true\``. A name that
        // has no binding to resolve against must not go looking for one.
        assert_eq!(just_kinds("true"), vec![Token::True]);
        assert_eq!(just_kinds("false"), vec![Token::False]);
        // A longer identifier that merely STARTS with one is still an
        // identifier — logos's longest-match, and the reason a column called
        // `truestory` is not two tokens.
        assert_eq!(just_kinds("truestory"), vec![Token::Ident]);
        assert_eq!(just_kinds("falsey"), vec![Token::Ident]);
        assert_eq!(just_kinds("is_true"), vec![Token::Ident]);
    }

    #[test]
    fn in_use_as_iri_and_prefix_are_ordinary_identifiers() {
        // Five words this lexer used to reserve. The named-graph clause and
        // the import took the first three, and both forms went — so a table
        // with an `in` column, or a binding called `use`, parses like any
        // other name. `iri` went with `@subject(iri = …)`: it
        // named an argument of a form that is now an assignment. `prefix` went
        // with the CURIE (grammar.bnf, § RESERVED KEYWORDS).
        assert_eq!(just_kinds("in"), vec![Token::Ident]);
        assert_eq!(just_kinds("use"), vec![Token::Ident]);
        assert_eq!(just_kinds("as"), vec![Token::Ident]);
        assert_eq!(just_kinds("iri"), vec![Token::Ident]);
        assert_eq!(just_kinds("prefix"), vec![Token::Ident]);
    }

    #[test]
    fn the_twelve_catalogue_words_are_ordinary_identifiers() {
        // They are NOT keywords (grammar.bnf, § RESERVED KEYWORDS), and it is
        // load-bearing: the verbs stopped being grammar, and a keyword IS
        // grammar. Reserving any of these in the lexer would undo that one
        // layer below the layer that decided it.
        for word in [
            "where", "select", "join", "on", "io", "str", "seq", "parse", "clean", "validate",
            "math", "anon",
        ] {
            assert_eq!(
                just_kinds(word),
                vec![Token::Ident],
                "`{word}` must lex as an IDENT"
            );
        }
    }

    // ─── Disambiguation / longest-match guards ────────────────────────

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
    fn a_chain_of_pipes_lexes_no_pipe_at_all() {
        // `|` matches no rule, so each one is dropped by the lossy pass and only
        // the `>` survives as `Gt`. The parser sees the `|` — the LOSSLESS pass
        // keeps it — and refuses the form by name.
        let kinds = just_kinds("a |> b |> c");
        let non_trivia: Vec<_> = kinds
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace))
            .collect();
        assert_eq!(
            non_trivia,
            vec![
                Token::Ident,
                Token::Gt,
                Token::Ident,
                Token::Gt,
                Token::Ident,
            ]
        );
    }

    #[test]
    fn precedence_walk_lexes_every_operator() {
        // Covers the precedence walk (grammar.bnf, § OPERATOR PRECEDENCE TABLE):
        // `a or b and c == d + e * f`.
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
