//! `textDocument/semanticTokens/full` — full-document semantic tokens.
//!
//! # Why this is load-bearing for the playground
//!
//! Monaco renders the playground **inert** (no syntax coloring) without LSP
//! semantic tokens: Fossil ships no `TextMate` grammar to the *playground* —
//! the browser editor relies on the LSP `semanticTokensProvider`. So this module
//! is the *only* source of syntax highlighting in the v0.1 playground.
//!
//! # Shape (LSP spec)
//!
//! The provider declares a **legend** ([`semantic_legend`]) — an ordered list of
//! token *types* and *modifiers*. Tokens themselves are a flat `Vec<u32>` of
//! 5-tuples `(deltaLine, deltaStartChar, length, tokenType, tokenModifiers)`,
//! delta-encoded relative to the previous token (the LSP wire format). The
//! `tokenType` is an *index* into the legend's `token_types`; `length` and
//! `deltaStartChar` are **UTF-16 code units** (Monaco counts UTF-16), so we route
//! every column / length through the [`crate::line_index::LineIndex`]
//! — without it any source with a multi-byte character
//! (non-ASCII IRIs, emoji in comments) colors the wrong span.
//!
//! # FILE-keyed, WASM-clean
//!
//! [`semantic_tokens`] is a whole-file CST walk — it re-runs **once** per edit
//! (no per-mapping Salsa key, so `MAX_PER_MAPPING_FAN_OUT` is untouched). It is a
//! pure function over [`fossil_syntax::parse`] + the line index, with no native
//! dependency, so `fossil-ide` stays inside the WASM gate. We emit
//! full-document tokens rather than delta or range ones: delta is an
//! optimisation, and the whole-file walk is not the cost on a program's scale.

use fossil_base::SourceFile;
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use rowan::WalkEvent;

use crate::line_index::LineIndex;
use crate::position::line_index;

/// Token-type indices into [`semantic_legend`]'s `token_types`. The numeric
/// value IS the `tokenType` field of the emitted 5-tuples, so the order here
/// MUST stay in lock-step with [`LEGEND_TYPES`] — and with the third table,
/// [`legend_type_name`], which renders the same indices back for review.
///
/// `tests/semantic_legend.rs` holds all three: it scrapes the names out of this
/// module (they are `pub(super)`, so the name is reachable nowhere else) and
/// calls the other two. It was prose, and prose does not go red — permuting any
/// two of these left every test in the tree green.
mod ty {
    pub(super) const KEYWORD: u32 = 0;
    pub(super) const NAMESPACE: u32 = 1;
    pub(super) const TYPE: u32 = 2;
    pub(super) const FUNCTION: u32 = 3;
    pub(super) const PROPERTY: u32 = 4;
    pub(super) const STRING: u32 = 5;
    pub(super) const NUMBER: u32 = 6;
    pub(super) const OPERATOR: u32 = 7;
    pub(super) const COMMENT: u32 = 8;
    pub(super) const VARIABLE: u32 = 9;
}

/// The legend's token *types*, in index order (index == the `tokenType` u32).
/// The minimal v0.1 set Fossil needs for legible coloring: keyword, namespace,
/// type, function, property, string, number, operator, comment, variable.
const LEGEND_TYPES: [SemanticTokenType; 10] = [
    SemanticTokenType::KEYWORD,
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::TYPE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::COMMENT,
    SemanticTokenType::VARIABLE,
];

/// The LSP semantic-tokens legend Fossil's `semanticTokensProvider` declares.
///
/// `fossil-lsp` plugs this straight into
/// `SemanticTokensOptions { legend: fossil_ide::semantic_legend(), .. }` when
/// registering the capability. v0.1
/// emits no modifiers (an empty modifier list), so the `tokenModifiers` bitset
/// of every emitted token is `0`.
#[must_use]
pub fn semantic_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: LEGEND_TYPES.to_vec(),
        token_modifiers: Vec::<SemanticTokenModifier>::new(),
    }
}

/// A decoded, absolute-positioned token, before delta-encoding. Internal —
/// the public surface is the delta-encoded `Vec<u32>`. `line`/`start_char` are
/// UTF-16 coordinates; `length` is a UTF-16 code-unit count.
#[derive(Debug, Clone, Copy)]
struct AbsToken {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
}

/// Full-document semantic tokens for `file`, delta-encoded per the LSP wire
/// format: a flat `Vec<u32>` of 5-tuples
/// `(deltaLine, deltaStartChar, length, tokenType, tokenModifiers)`.
///
/// Walks the [`fossil_syntax::parse`] CST in source order, classifies each
/// *leaf* token via [`classify`], and converts byte offsets to UTF-16
/// `(line, char, length)` through the FILE-keyed [`LineIndex`]. Tokens with no
/// semantic category (whitespace, structural punctuation, parse-error trivia)
/// are skipped. The result is exactly what
/// `textDocument/semanticTokens/full` returns; `fossil-lsp` wraps it in
/// `SemanticTokens { result_id: None, data }`.
#[must_use]
pub fn semantic_tokens(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<u32> {
    let cst = fossil_syntax::parse(db, file);
    let root: SyntaxNode = cst.root(db).syntax();
    let index = line_index(db, file);

    let mut abs: Vec<AbsToken> = Vec::new();
    // Pre-order walk: `WalkEvent::Enter` on every node; we only act on tokens,
    // which we reach by enumerating each node's direct child tokens. Using a
    // pre-order element walk keeps tokens in strict source order (required for
    // correct delta encoding).
    for event in root.preorder_with_tokens() {
        if let WalkEvent::Enter(rowan::NodeOrToken::Token(tok)) = event
            && let Some(token_type) = classify(&tok)
        {
            push_token(&mut abs, &index, &tok, token_type);
        }
    }

    delta_encode(&abs)
}

/// Classify a leaf [`SyntaxToken`] into a legend token-type index, or `None`
/// if it carries no color (whitespace, structural punctuation, indent/dedent,
/// errors). An `IDENT` is disambiguated by its local tree shape — see
/// [`ident_type`] for the three it can take.
fn classify(tok: &SyntaxToken) -> Option<u32> {
    use SyntaxKind as K;
    match tok.kind() {
        // ── unambiguous lexical classes ───────────────────────────────
        K::COMMENT => Some(ty::COMMENT),
        // A string with a hole is carved into a run of tokens, so every part
        // of it has to be named here or the literal loses its colour halfway
        // through — which is what happened when the carve landed.
        K::STRING | K::STRING_OPEN | K::STRING_TEXT | K::STRING_CLOSE => Some(ty::STRING),
        K::INTEGER | K::FLOAT => Some(ty::NUMBER),
        // `K::ABS_IRI => NAMESPACE` was here, and `K::TEMPLATE` shared the
        // STRING arm above. Neither is a token: `<` and `>` have one reading
        // each, and a constant IRI is a STRING like any other.

        // ── keywords (from / and / or / not) + the `@attr` marker,
        //    read as a keyword ─────────────────────────────────────────────
        //
        // `in`, `use` and `as` were here. They stopped being keywords when the
        // named-graph clause and the import left the grammar, and an `in`
        // painted as a keyword would now be a lie about an ordinary column.
        // `iri` went the same way: the subject slot is `@subject`
        // and `iri` is an ordinary identifier again, so painting it as a
        // keyword would colour a user's column name. `prefix` is the fifth and
        // the most recent (grammar.bnf, § RESERVED KEYWORDS).
        K::KW_FROM
        | K::KW_AND
        | K::KW_OR
        | K::KW_NOT
        | K::AT_ATTR => Some(ty::KEYWORD),

        // ── operators (assignment, ternary, arithmetic, comparison) ───────
        //
        // `K::PIPE` was the first name in this list. `|>` is not a token any
        // more (ruling 7 of 2026-08-11), so painting it
        // was painting a lexeme the lexer cannot produce.
        K::DEFINE
        | K::ASSIGN
        | K::EQ
        | K::NEQ
        | K::LT
        | K::LE
        | K::GT
        | K::GE
        | K::PLUS
        | K::MINUS
        | K::STAR
        | K::SLASH
        | K::PERCENT
        | K::T_QUESTION
        // The hole's opener: an operator, because it is what separates the
        // expression inside from the text around it.
        | K::INTERP_OPEN => Some(ty::OPERATOR),

        // ── context-sensitive names ───────────────────────────────────
        K::IDENT => Some(ident_type(tok)),

        // The hole's CLOSER, and the reason it needs its own arm: `{` is
        // `INTERP_OPEN`, a token of its own, but `}` is an ordinary `RBRACE`
        // shared with `type { Person } := …` and `{ A, B } := …`. Painting
        // every `RBRACE` would colour those, so the arm asks the parent — the
        // grammar puts the closer directly under `INTERPOLATION`
        // (`Interpolation := INTERP_OPEN Expression RBRACE`) and nowhere else.
        // Without it the literal opened as an operator and closed as nothing:
        // the run went string, string, operator, expression, *gap*, string.
        K::RBRACE if is_interpolation_close(tok) => Some(ty::OPERATOR),

        _ => None,
    }
}

/// Classify a bare `IDENT`. Calls and member access are `POSTFIX_EXPR` nodes —
/// there is no call *leaf* token. So we read the IDENT's local tree shape, in
/// priority order:
///
/// 1. **property** — the IDENT names a record field: it directly follows a
///    `DOT` sibling (`User.name` member access under a `POSTFIX_EXPR`). It used
///    to have a second way in, the IDENT of a `FIELD_REF_EXPR` (`.name` in
///    primary position), and that node is gone — a leading `.` is an error —
///    which leaves this rule with ONE shape, and the one every reference now has.
/// 2. **function** — the IDENT is a *call callee*: its `LITERAL_EXPR` is the
///    first child of a `POSTFIX_EXPR` that also has an `LPAREN` child
///    (`upper(...)`, `io.csv(...)`).
/// 3. **variable** — otherwise (a mapping subject, a source name, a binding).
///
/// A **namespace** rule sat between 2 and 3: the prefix segment of `ex:Person`,
/// found by climbing to an `IRI_EXPR`. Both are gone, and a shape name is now
/// an ordinary IDENT that falls to rule 3 — correctly, because it IS a binding
/// the program made.
fn ident_type(tok: &SyntaxToken) -> u32 {
    if is_field_name(tok) {
        return ty::PROPERTY;
    }
    if is_call_callee(tok) {
        return ty::FUNCTION;
    }
    ty::VARIABLE
}

/// Whether this `RBRACE` closes an interpolation hole rather than a
/// destructuring pattern. The parser builds `INTERPOLATION` around
/// `INTERP_OPEN Expression RBRACE` and bumps the closer as a direct child of
/// that node, so the parent is the whole test — and it stays true for the
/// recovery path, where `expect_or_recover` still attaches the brace it found
/// before `p.finish()`.
fn is_interpolation_close(tok: &SyntaxToken) -> bool {
    tok.parent()
        .is_some_and(|p| p.kind() == SyntaxKind::INTERPOLATION)
}

/// Whether `tok` names a record field — an IDENT immediately preceded by a
/// `DOT` token (member access).
fn is_field_name(tok: &SyntaxToken) -> bool {
    prev_token_kind(tok) == Some(SyntaxKind::DOT)
}

/// Whether `tok` is the callee of a call — its primary node is the first child of
/// a `POSTFIX_EXPR` that has an `LPAREN` child (a function application).
fn is_call_callee(tok: &SyntaxToken) -> bool {
    let Some(primary) = tok.parent() else {
        return false;
    };
    if primary.kind() != SyntaxKind::LITERAL_EXPR {
        return false;
    }
    let Some(postfix) = primary.parent() else {
        return false;
    };
    if postfix.kind() != SyntaxKind::POSTFIX_EXPR {
        return false;
    }
    // The primary must be the first child (the callee, not an argument), and the
    // POSTFIX must carry a call `(`.
    let is_callee = postfix.first_child().is_some_and(|c| c == primary);
    let has_call_paren = postfix
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .any(|t| t.kind() == SyntaxKind::LPAREN);
    is_callee && has_call_paren
}

/// The `SyntaxKind` of the token immediately preceding `tok` in source order,
/// skipping whitespace/newlines, or `None` if there is no prior token.
fn prev_token_kind(tok: &SyntaxToken) -> Option<SyntaxKind> {
    let mut cur = tok.prev_token();
    while let Some(t) = cur {
        if !matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) {
            return Some(t.kind());
        }
        cur = t.prev_token();
    }
    None
}

/// Convert one token's byte range to UTF-16 `(line, start_char, length)` via the
/// [`LineIndex`] and push an [`AbsToken`]. A token that spans multiple lines
/// (only multi-line strings/comments in practice) is recorded on its start line
/// with its first-line length — adequate for v0.1 coloring; Monaco tolerates a
/// token that does not reach the line end.
fn push_token(abs: &mut Vec<AbsToken>, index: &LineIndex, tok: &SyntaxToken, token_type: u32) {
    let range = tok.text_range();
    let start_byte = u32::from(range.start());
    let end_byte = u32::from(range.end());
    let start = index.position(start_byte);
    let end = index.position(end_byte);
    let length = if end.line == start.line {
        end.character.saturating_sub(start.character)
    } else {
        // Multi-line token: color to the end of the first line. UTF-16 length of
        // the first line's slice == (next line start as col) is unknown here, so
        // approximate with the token's own first-line UTF-16 width via the text.
        utf16_first_line_len(tok)
    };
    if length == 0 {
        return;
    }
    abs.push(AbsToken {
        line: start.line,
        start_char: start.character,
        length,
        token_type,
    });
}

/// UTF-16 length of a token's first source line (for multi-line tokens).
fn utf16_first_line_len(tok: &SyntaxToken) -> u32 {
    let text = tok.text();
    let first = text.split('\n').next().unwrap_or(text);
    u32::try_from(first.chars().map(char::len_utf16).sum::<usize>()).unwrap_or(u32::MAX)
}

/// Delta-encode absolute tokens into the LSP flat `Vec<u32>` 5-tuple stream.
/// `deltaLine` is relative to the previous token's line; `deltaStartChar` is
/// relative to the previous token's start *when on the same line*, else absolute.
/// `tokenModifiers` is always `0` (v0.1 emits no modifiers).
fn delta_encode(abs: &[AbsToken]) -> Vec<u32> {
    let mut data = Vec::with_capacity(abs.len() * 5);
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for t in abs {
        let delta_line = t.line - prev_line;
        let delta_start = if delta_line == 0 {
            t.start_char - prev_start
        } else {
            t.start_char
        };
        data.extend_from_slice(&[delta_line, delta_start, t.length, t.token_type, 0]);
        prev_line = t.line;
        prev_start = t.start_char;
    }
    data
}

/// Decode a flat 5-tuple stream back into absolute `(line, col, len, type)` rows.
///
/// Public so the snapshot test (and a future LSP-side assertion) can produce a
/// stable, human-reviewable view of the token stream.
#[must_use]
pub fn decode_tokens(data: &[u32]) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    let mut line = 0u32;
    let mut col = 0u32;
    for chunk in data.chunks_exact(5) {
        let (dl, dc, len, ty) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        if dl == 0 {
            col += dc;
        } else {
            line += dl;
            col = dc;
        }
        out.push((line, col, len, ty));
    }
    out
}

/// Human-readable legend type name for a `tokenType` index — used by the
/// snapshot test to render `(line, col, len, "keyword")` rows instead of opaque
/// numeric type ids, so a snapshot diff is reviewable.
#[must_use]
pub const fn legend_type_name(token_type: u32) -> &'static str {
    match token_type {
        ty::KEYWORD => "keyword",
        ty::NAMESPACE => "namespace",
        ty::TYPE => "type",
        ty::FUNCTION => "function",
        ty::PROPERTY => "property",
        ty::STRING => "string",
        ty::NUMBER => "number",
        ty::OPERATOR => "operator",
        ty::COMMENT => "comment",
        ty::VARIABLE => "variable",
        _ => "unknown",
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn db_file(src: &str) -> (fossil_base::FossilDb, SourceFile) {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    /// v0.1 emits no modifiers, so the `tokenModifiers` bitset of every token is
    /// `0`. The types are `tests/semantic_legend.rs`'s subject, not this one's:
    /// this counted to ten and pinned indices 0 and 8, which is a copy of two
    /// tenths of the answer and could not notice the other eight moving.
    #[test]
    fn the_legend_declares_no_modifiers() {
        assert!(semantic_legend().token_modifiers.is_empty());
    }

    /// A whole small program, so the token stream is the one a real file emits.
    /// These fixtures were `prefix ex: <https://example.org/>` — one retired
    /// line, which lexes to error tokens now and asserts nothing about colour.
    const SRC: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
Users : Person from users
    @subject = \"https://example.org/u/{users.id}\"
";

    #[test]
    fn tokens_are_a_multiple_of_five() {
        let (db, file) = db_file(SRC);
        let data = semantic_tokens(&db, file);
        assert_eq!(data.len() % 5, 0, "the token stream must be 5-tuples");
        assert!(!data.is_empty(), "a program should emit tokens");
    }

    /// The reserved set is `from`, `and`, `or`, `not` and the `@attr` sigils;
    /// painting an ordinary identifier as a keyword is the failure the
    /// classifier's own comment warns about.
    #[test]
    fn from_is_painted_as_a_keyword() {
        let (db, file) = db_file(SRC);
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        let kw = decoded
            .iter()
            .find(|&&(_, _, _, t)| t == ty::KEYWORD)
            .expect("a keyword token");
        assert_eq!(kw.0, 2, "`from` is on the mapping header line");
        assert_eq!(kw.2, 4, "len of `from`");
    }

    /// The hole closes in the colour it opened in.
    ///
    /// `{` is `INTERP_OPEN`, a token of its own and never anything else, so it
    /// was painted from the day the carve landed. `}` is an ordinary `RBRACE`,
    /// the same token the destructuring on line 0 writes, and it was painted as
    /// nothing at all — a literal that opened as an operator and ended in a
    /// gap. Both halves are asserted here, because the fix has to name the
    /// interpolation and not the brace: line 3 gains the closer, line 0 keeps
    /// exactly the one operator it always had, its `:=`.
    #[test]
    fn the_interpolation_closes_in_the_colour_it_opened() {
        let (db, file) = db_file(SRC);
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        let ops_on = |want: u32| -> Vec<u32> {
            decoded
                .iter()
                .filter(|&&(line, _, _, t)| line == want && t == ty::OPERATOR)
                .map(|&(_, col, _, _)| col)
                .collect()
        };
        // `    @subject = "https://example.org/u/{users.id}"`
        //                ^13            the hole ^38    ^47
        assert_eq!(ops_on(3), vec![13, 38, 47], "all tokens: {decoded:?}");
        // `type { Person } := io.shex("person.shex")` — the `:=` and NOTHING
        // else. If the classifier had taken every `RBRACE`, col 14 would be here.
        assert_eq!(ops_on(0), vec![16], "all tokens: {decoded:?}");
    }

    #[test]
    fn comment_classified_as_comment() {
        // Fossil comments are `//`-to-EOL (lexer.rs), NOT `#`.
        let (db, file) = db_file(&format!("// a comment\n{SRC}"));
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        assert!(
            decoded.iter().any(|&(_, _, _, t)| t == ty::COMMENT),
            "expected a comment token: {decoded:?}"
        );
    }

    #[test]
    fn delta_encoding_is_relative() {
        // Two tokens on the same line: the second's deltaStartChar is relative.
        let abs = [
            AbsToken {
                line: 0,
                start_char: 0,
                length: 6,
                token_type: ty::KEYWORD,
            },
            AbsToken {
                line: 0,
                start_char: 7,
                length: 2,
                token_type: ty::NAMESPACE,
            },
        ];
        let data = delta_encode(&abs);
        // token 2: deltaLine 0, deltaStart 7-0=7.
        assert_eq!(&data[5..10], &[0, 7, 2, ty::NAMESPACE, 0]);
    }

    #[test]
    fn utf16_columns_for_multibyte_comment() {
        // A comment with a 2-byte `é`; the token AFTER it on the next line must
        // start at a UTF-16-correct column (the LineIndex handles this). Read as
        // bytes, `café` is five and the column would be off by one.
        let (db, file) = db_file(&format!("// café\n{SRC}"));
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        let first_on_line_1 = decoded
            .iter()
            .find(|&&(line, _, _, _)| line == 1)
            .expect("a token on the line after the comment");
        assert_eq!(
            first_on_line_1.1, 0,
            "the line after the comment starts at column 0"
        );
    }
}
