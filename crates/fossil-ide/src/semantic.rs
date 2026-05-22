//! `textDocument/semanticTokens/full` — full-document semantic tokens (SC#4).
//!
//! # Why this is load-bearing for the playground
//!
//! Monaco renders the playground **inert** (no syntax coloring) without LSP
//! semantic tokens: Fossil ships no `TextMate` grammar to the *playground* (the
//! `VS Code` extension gets one in Phase 9; the browser editor relies on the
//! LSP `semanticTokensProvider`). So this module is the *only* source of syntax
//! highlighting in the v0.1 playground (Research §Semantic Tokens / Monaco).
//!
//! # Shape (LSP spec)
//!
//! The provider declares a **legend** ([`semantic_legend`]) — an ordered list of
//! token *types* and *modifiers*. Tokens themselves are a flat `Vec<u32>` of
//! 5-tuples `(deltaLine, deltaStartChar, length, tokenType, tokenModifiers)`,
//! delta-encoded relative to the previous token (the LSP wire format). The
//! `tokenType` is an *index* into the legend's `token_types`; `length` and
//! `deltaStartChar` are **UTF-16 code units** (Monaco counts UTF-16), so we route
//! every column / length through the 06-05 [`crate::line_index::LineIndex`]
//! (Research Pitfall #4) — without it any source with a multi-byte character
//! (non-ASCII IRIs, emoji in comments) colors the wrong span.
//!
//! # FILE-keyed, WASM-clean (Research Pitfall #3)
//!
//! [`semantic_tokens`] is a whole-file CST walk — it re-runs **once** per edit
//! (no per-mapping Salsa key, so `MAX_PER_MAPPING_FAN_OUT` is untouched). It is a
//! pure function over [`fossil_syntax::parse`] + the line index, with no native
//! dependency, so `fossil-ide` stays inside the 9-crate WASM gate. We emit
//! full-document tokens (not delta/range) per Research's v0.1 recommendation:
//! < 5ms on 200 lines is well within the perf budget; delta is a v2 optimization.

use fossil_base::SourceFile;
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use rowan::WalkEvent;

use crate::line_index::LineIndex;
use crate::position::line_index;

/// Token-type indices into [`semantic_legend`]'s `token_types`. The numeric
/// value IS the `tokenType` field of the emitted 5-tuples, so the order here
/// MUST stay in lock-step with [`LEGEND_TYPES`].
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
/// The minimal v0.1 set Fossil needs for legible coloring (Research §Semantic
/// Tokens): keyword, namespace, type, function, property, string, number,
/// operator, comment, variable.
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
/// `fossil-lsp` (06-08) plugs this straight into
/// `SemanticTokensOptions { legend: fossil_ide::semantic_legend(), .. }` when
/// registering the capability (Research §LSP capability registration). v0.1
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
/// *leaf* token via [`classify`] (using its parent node for the IDENT /
/// prefixed-name disambiguation), and converts byte offsets to UTF-16
/// `(line, char, length)` through the FILE-keyed [`LineIndex`]. Tokens with no
/// semantic category (whitespace, structural punctuation, parse-error trivia)
/// are skipped. The result is exactly what
/// `textDocument/semanticTokens/full` returns; `fossil-lsp` (06-08) wraps it in
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
/// errors). Uses the token's parent node kind to disambiguate `IDENT` /
/// `PREFIXED_NAME` (a mapping subject vs a shape type vs a stdlib call).
fn classify(tok: &SyntaxToken) -> Option<u32> {
    use SyntaxKind as K;
    match tok.kind() {
        // ── unambiguous lexical classes ───────────────────────────────
        K::COMMENT => Some(ty::COMMENT),
        K::STRING | K::TEMPLATE => Some(ty::STRING),
        K::INTEGER | K::FLOAT => Some(ty::NUMBER),
        K::FIELD_REF => Some(ty::PROPERTY),
        K::ABS_IRI => Some(ty::NAMESPACE),
        K::ENV_VAR => Some(ty::VARIABLE),

        // ── keywords (prefix / from / in / use / as / and / or / not /
        //    iri) + the @export / @attr annotation markers read as keywords ─
        K::KW_PREFIX
        | K::KW_FROM
        | K::KW_IN
        | K::KW_USE
        | K::KW_AS
        | K::KW_AND
        | K::KW_OR
        | K::KW_NOT
        | K::KW_IRI
        | K::AT_EXPORT
        | K::AT_ATTR => Some(ty::KEYWORD),

        // ── operators (pipeline, assignment, ternary, arithmetic,
        //    comparison, type-annotation `::`, shape `&`) ─────────────────
        K::PIPE
        | K::ARROW
        | K::DEFINE
        | K::ASSIGN
        | K::TYPE_ANNOT
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
        | K::SHAPE_AND => Some(ty::OPERATOR),

        // ── context-sensitive names ───────────────────────────────────
        K::PREFIXED_NAME => Some(prefixed_name_type(tok)),
        K::IDENT => Some(ident_type(tok)),

        _ => None,
    }
}

/// A `PREFIXED_NAME` (`ex:Person`, `ex:name`) is a shape *type* when it sits in
/// a mapping header's `SHAPE_EXPR` (`User : ex:Person`), and a *property*
/// otherwise (the predicate of a `PROPERTY`, e.g. `ex:name = .name`). Default to
/// namespace if it is neither (a bare prefixed name in an expression position).
fn prefixed_name_type(tok: &SyntaxToken) -> u32 {
    if has_ancestor(tok, SyntaxKind::SHAPE_EXPR) {
        ty::TYPE
    } else if has_ancestor(tok, SyntaxKind::PROPERTY_LHS) || has_ancestor(tok, SyntaxKind::PROPERTY)
    {
        ty::PROPERTY
    } else {
        ty::NAMESPACE
    }
}

/// Classify a bare `IDENT`. The grammar (parser/expr.rs) produces no `CALL_EXPR`
/// / `FIELD_REF` *leaf* — calls and member access are `POSTFIX_EXPR` nodes and a
/// primary field ref is a `FIELD_REF_EXPR`. So we read the IDENT's local tree
/// shape, in priority order:
///
/// 1. **property** — the IDENT names a record field: it is the IDENT of a
///    `FIELD_REF_EXPR` (`.name` in primary position) or it directly follows a
///    `DOT` sibling (`x.name` member access under a `POSTFIX_EXPR`).
/// 2. **function** — the IDENT is a *call callee*: its primary node
///    (`LITERAL_EXPR` / `IRI_EXPR`) is the first child of a `POSTFIX_EXPR` that
///    also has an `LPAREN` child (`upper(...)`, `io.csv(...)`).
/// 3. **namespace** — the prefix segment of a prefixed name (`ex` in
///    `ex:Person`): under an `IRI_EXPR`.
/// 4. **variable** — otherwise (a mapping subject, a source name, a binding).
fn ident_type(tok: &SyntaxToken) -> u32 {
    if is_field_name(tok) {
        return ty::PROPERTY;
    }
    if is_call_callee(tok) {
        return ty::FUNCTION;
    }
    if has_ancestor(tok, SyntaxKind::IRI_EXPR) {
        return ty::NAMESPACE;
    }
    ty::VARIABLE
}

/// Whether `tok` names a record field — the IDENT of a `FIELD_REF_EXPR` or an
/// IDENT immediately preceded by a `DOT` token (member access).
fn is_field_name(tok: &SyntaxToken) -> bool {
    if tok
        .parent()
        .is_some_and(|p| p.kind() == SyntaxKind::FIELD_REF_EXPR)
    {
        return true;
    }
    // Preceding sibling token is a DOT (postfix `x.name`).
    prev_token_kind(tok) == Some(SyntaxKind::DOT)
}

/// Whether `tok` is the callee of a call — its primary node is the first child of
/// a `POSTFIX_EXPR` that has an `LPAREN` child (a function application).
fn is_call_callee(tok: &SyntaxToken) -> bool {
    let Some(primary) = tok.parent() else {
        return false;
    };
    if !matches!(
        primary.kind(),
        SyntaxKind::LITERAL_EXPR | SyntaxKind::IRI_EXPR
    ) {
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

/// Whether `tok` has an ancestor node of `kind`.
fn has_ancestor(tok: &SyntaxToken, kind: SyntaxKind) -> bool {
    let mut node = tok.parent();
    while let Some(n) = node {
        if n.kind() == kind {
            return true;
        }
        node = n.parent();
    }
    false
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
        let db = fossil_base::FossilDb::new(system);
        let file = SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        (db, file)
    }

    #[test]
    fn legend_has_ten_types_and_no_modifiers() {
        let legend = semantic_legend();
        assert_eq!(legend.token_types.len(), 10);
        assert!(legend.token_modifiers.is_empty());
        assert_eq!(
            legend.token_types[ty::KEYWORD as usize],
            SemanticTokenType::KEYWORD
        );
        assert_eq!(
            legend.token_types[ty::COMMENT as usize],
            SemanticTokenType::COMMENT
        );
    }

    #[test]
    fn tokens_are_a_multiple_of_five() {
        let (db, file) = db_file("prefix ex: <https://example.org/>\n");
        let data = semantic_tokens(&db, file);
        assert_eq!(data.len() % 5, 0, "the token stream must be 5-tuples");
        assert!(!data.is_empty(), "a prefix decl should emit tokens");
    }

    #[test]
    fn prefix_keyword_is_first_token() {
        let (db, file) = db_file("prefix ex: <https://example.org/>\n");
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        // The `prefix` keyword is at line 0, col 0, length 6, type keyword.
        let first = decoded.first().expect("at least one token");
        assert_eq!(first.0, 0, "line");
        assert_eq!(first.1, 0, "col");
        assert_eq!(first.2, 6, "len of `prefix`");
        assert_eq!(first.3, ty::KEYWORD, "type keyword");
    }

    #[test]
    fn comment_classified_as_comment() {
        // Fossil comments are `//`-to-EOL (lexer.rs), NOT `#`.
        let (db, file) = db_file("// a comment\nprefix ex: <https://example.org/>\n");
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
        // start at a UTF-16-correct column (the LineIndex handles this).
        let (db, file) = db_file("// café\nprefix ex: <https://example.org/>\n");
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        // `prefix` on line 1, col 0.
        let kw = decoded
            .iter()
            .find(|&&(_, _, _, t)| t == ty::KEYWORD)
            .expect("a keyword token");
        assert_eq!(kw.0, 1, "prefix is on line 1");
        assert_eq!(kw.1, 0, "prefix starts at col 0");
    }
}
