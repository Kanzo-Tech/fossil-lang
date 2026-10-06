//! `textDocument/semanticTokens/full` — full-document semantic tokens, and
//! the same answer as absolute spans for the browser.
//!
//! # Why this exists
//!
//! Fossil ships no `TextMate` grammar, so an LSP client colours a program from
//! these or not at all. The `CodeMirror` layer (`@fossil-lang/codemirror-fossil`)
//! paints from `tokenize()` first and lays [`semantic_spans`] over it — the
//! rust-analyzer-over-TextMate arrangement — because the lexer cannot tell a
//! shape from a binding from a column, and this module can.
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
//! # What a name is
//!
//! A CST walk plus two things the database already knows: the names the file's
//! `type { … } := …` bindings introduce and the references its bindings read
//! (both from [`fossil_hir::def_map::def_map`]), and which dotted heads are the
//! catalogue's ([`fossil_hir::stdlib`]). Nothing here infers a type; that is
//! hover's job and costs a typecheck.
//!
//! # FILE-keyed, WASM-clean
//!
//! [`semantic_spans`] re-runs **once** per edit (no per-mapping Salsa key, so
//! `MAX_PER_MAPPING_FAN_OUT` is untouched): one CST walk over
//! [`fossil_syntax::parse`] and the signatures-only `def_map`, with no native
//! dependency, so `fossil-ide` stays inside the WASM gate.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use rowan::WalkEvent;
use smol_str::SmolStr;

use crate::line_index::LineIndex;
use crate::position::line_index;

/// Token-type indices into [`semantic_legend`]'s `token_types`. The numeric
/// value IS the `tokenType` field of the emitted 5-tuples, so the order here
/// MUST stay in lock-step with [`LEGEND_TYPES`] — and with the third table,
/// [`legend_type_name`], which renders the same indices back for review.
///
/// `tests/semantic_legend.rs` holds all three: it scrapes the names out of this
/// module (they are `pub(super)`, so the name is reachable nowhere else) and
/// calls the other two. A new type is APPENDED: a client that cached the legend
/// reads every index before it unchanged.
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
    pub(super) const PARAMETER: u32 = 10;
}

/// Token-modifier BIT indices into [`semantic_legend`]'s `token_modifiers`:
/// modifier `i` is bit `1 << i` of the emitted `tokenModifiers`. Held against
/// [`LEGEND_MODIFIERS`] and [`legend_modifier_name`] by the same guard as
/// [`ty`].
mod md {
    pub(super) const DECLARATION: u32 = 0;
}

/// The legend's token *types*, in index order (index == the `tokenType` u32).
const LEGEND_TYPES: [SemanticTokenType; 11] = [
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
    SemanticTokenType::PARAMETER,
];

/// The legend's token *modifiers*, in bit order.
const LEGEND_MODIFIERS: [SemanticTokenModifier; 1] = [SemanticTokenModifier::DECLARATION];

/// The bitset for a token carrying `declaration`.
const DECLARATION: u32 = 1 << md::DECLARATION;

/// The LSP semantic-tokens legend Fossil's `semanticTokensProvider` declares.
///
/// `fossil-lsp` plugs this straight into
/// `SemanticTokensOptions { legend: fossil_ide::semantic_legend(), .. }` when
/// registering the capability.
#[must_use]
pub fn semantic_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: LEGEND_TYPES.to_vec(),
        token_modifiers: LEGEND_MODIFIERS.to_vec(),
    }
}

/// One classified span, file-absolute in BYTES — what [`semantic_tokens`]
/// delta-encodes and what `fossil-wasm` hands a browser editor as a range.
///
/// A span is not always a whole token: a connection reference is carved out
/// of the string literal it is written in, so `"@warehouse/x.csv"` is three
/// spans — string, namespace, string. Spans are in source order and never
/// overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSpan {
    /// Byte range into the file's text.
    pub range: Range<u32>,
    /// Index into the legend's `token_types` — [`legend_type_name`] reads it.
    pub token_type: u32,
    /// Bitset over the legend's `token_modifiers` — [`legend_modifier_name`]
    /// reads each bit.
    pub modifiers: u32,
}

/// Every classified span in `file`, in source order.
///
/// Tokens with no semantic category (whitespace, structural punctuation,
/// parse-error trivia) and literals the lexer already names exactly (`true`,
/// `false`, `null`) are skipped.
#[must_use]
pub fn semantic_spans(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<SemanticSpan> {
    let cst = fossil_syntax::parse(db, file);
    let root: SyntaxNode = cst.root(db).syntax();
    let names = FileNames::of(db, file);

    let mut spans = Vec::new();
    for event in root.preorder_with_tokens() {
        let WalkEvent::Enter(rowan::NodeOrToken::Token(tok)) = event else {
            continue;
        };
        if let Some(alias) = connection_alias(&tok, &names) {
            let range = byte_range(&tok);
            let (at, past) = (alias.start, alias.end);
            spans.push(span(range.start..at, ty::STRING, 0));
            spans.push(span(alias, ty::NAMESPACE, 0));
            spans.push(span(past..range.end, ty::STRING, 0));
        } else if let Some((token_type, modifiers)) = classify(&tok, &names) {
            spans.push(span(byte_range(&tok), token_type, modifiers));
        }
    }
    spans
}

/// Full-document semantic tokens for `file`, delta-encoded per the LSP wire
/// format: a flat `Vec<u32>` of 5-tuples
/// `(deltaLine, deltaStartChar, length, tokenType, tokenModifiers)`.
///
/// [`semantic_spans`] through the FILE-keyed [`LineIndex`]. The result is
/// exactly what `textDocument/semanticTokens/full` returns; `fossil-lsp` wraps
/// it in `SemanticTokens { result_id: None, data }`.
#[must_use]
pub fn semantic_tokens(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<u32> {
    let index = line_index(db, file);
    let text = file.text(db);
    let abs: Vec<AbsToken> = semantic_spans(db, file)
        .into_iter()
        .filter_map(|s| abs_token(&index, text, &s))
        .collect();
    delta_encode(&abs)
}

/// What the file itself declares, read once per walk.
struct FileNames {
    /// The names `type { … } := …` introduced — a call to one of them is an
    /// edge to that shape, not a function.
    types: Vec<SmolStr>,
    /// Every string a binding reads as a reference: a source's URI, its
    /// `schema =` document, a `type` binding's document. These are the strings
    /// `fossil-lineage` reports and the locator resolves, so they are the only
    /// ones whose `@name/` prefix names a connection.
    references: Vec<SmolStr>,
}

impl FileNames {
    fn of(db: &dyn fossil_base::Db, file: SourceFile) -> Self {
        let def_map = fossil_hir::def_map::def_map(db, file);
        let types = def_map.types(db);
        let sources = def_map.sources(db);
        let references = sources
            .iter()
            .flat_map(|s| [s.uri.clone(), s.schema_arg.clone()])
            .chain(types.iter().map(|t| t.document.clone()))
            .flatten()
            .collect();
        Self {
            types: types.iter().map(|t| t.name.clone()).collect(),
            references,
        }
    }

    fn is_type(&self, name: &str) -> bool {
        self.types.iter().any(|t| t == name)
    }
}

const fn span(range: Range<u32>, token_type: u32, modifiers: u32) -> SemanticSpan {
    SemanticSpan {
        range,
        token_type,
        modifiers,
    }
}

fn byte_range(tok: &SyntaxToken) -> Range<u32> {
    let r = tok.text_range();
    u32::from(r.start())..u32::from(r.end())
}

/// The byte range of `@name` inside a string literal that a binding reads as a
/// reference, when the reference is written through a connection.
///
/// The split is [`fossil_locator::split_alias`]'s, the rule every other reader
/// of a reference uses; the range covers the `@` and stops before the `/`.
/// Only a binding's literals qualify — a mapping body's string is a value,
/// and an `@` at the start of one names nothing.
fn connection_alias(tok: &SyntaxToken, names: &FileNames) -> Option<Range<u32>> {
    if tok.kind() != SyntaxKind::STRING
        || tok
            .parent_ancestors()
            .any(|n| n.kind() == SyntaxKind::MAPPING)
    {
        return None;
    }
    let inner = tok.text().strip_prefix('"')?.strip_suffix('"')?;
    if !names.references.iter().any(|r| r == inner) {
        return None;
    }
    let (alias, _) = fossil_locator::split_alias(inner)?;
    let at = u32::from(tok.text_range().start()) + 1;
    let len = u32::try_from(alias.len() + 1).ok()?;
    Some(at..at + len)
}

/// Classify a leaf [`SyntaxToken`] into a legend `(type, modifiers)`, or `None`
/// if it carries no color (whitespace, structural punctuation, indent/dedent,
/// errors). An `IDENT` is disambiguated by its place in the tree — see
/// [`ident`].
fn classify(tok: &SyntaxToken, names: &FileNames) -> Option<(u32, u32)> {
    use SyntaxKind as K;
    let token_type = match tok.kind() {
        // ── unambiguous lexical classes ───────────────────────────────
        K::COMMENT => ty::COMMENT,
        // A string with a hole is carved into a run of tokens, so every part
        // of it has to be named here or the literal loses its colour halfway
        // through. A STRING after a member's `.` is not a literal: it is a
        // quoted member, `Knows."Person.id"`, coloured as the bare one is.
        K::STRING
            if tok.parent().is_some_and(|p| p.kind() == K::POSTFIX_EXPR)
                && follows_sibling(tok, K::DOT) =>
        {
            ty::PROPERTY
        }
        K::STRING | K::STRING_OPEN | K::STRING_TEXT | K::STRING_CLOSE => ty::STRING,
        K::INTEGER | K::FLOAT => ty::NUMBER,

        // ── the reserved words and the `@attr` marker ─────────────────
        //
        // `from`, `and`, `or`, `not` (grammar.bnf, § RESERVED KEYWORDS). The
        // contextual ones — `type` before a destructuring, `as` in a rename or
        // an alias — are IDENTs and are named by [`ident`] from their place.
        K::KW_FROM | K::KW_AND | K::KW_OR | K::KW_NOT | K::AT_ATTR => ty::KEYWORD,

        // ── operators (assignment, ternary, arithmetic, comparison) ───────
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
        | K::INTERP_OPEN => ty::OPERATOR,

        K::IDENT => return Some(ident(tok, names)),

        // The hole's CLOSER. `}` is an ordinary `RBRACE` shared with
        // `type { Person } := …` and `{ A, B } := …`, and the grammar puts the
        // interpolation's directly under `INTERPOLATION` and nowhere else.
        K::RBRACE if tok.parent().is_some_and(|p| p.kind() == K::INTERPOLATION) => {
            ty::OPERATOR
        }

        _ => return None,
    };
    Some((token_type, 0))
}

/// Classify an `IDENT` by the node it sits in.
///
/// | where | what |
/// |---|---|
/// | `type { Person, Order } := …` | `type` keyword, each member a type declaration |
/// | `Users : Person from …` | `Users` a variable declaration, `Person` a type |
/// | `users := …`, `{ A, B } := …` | a variable declaration each |
/// | `@rename(Person, "…" as foaf_name)` | `Person` a type, `as` keyword, `foaf_name` a property declaration |
/// | `Node as Other` | `Node` a variable, `as` keyword, `Other` a variable declaration |
/// | `email = …` in a body | a property — the shape's predicate |
/// | `delimiter = "|"` in a call | a parameter |
/// | `x.name` | a property, or a function when it is called: `io.csv(…)`, `User.where(…)` |
/// | a bare name | see [`bare_name`] |
fn ident(tok: &SyntaxToken, names: &FileNames) -> (u32, u32) {
    use SyntaxKind as K;
    let Some(parent) = tok.parent() else {
        return (ty::VARIABLE, 0);
    };
    match parent.kind() {
        _ if is_contextual_keyword(tok, parent.kind()) => (ty::KEYWORD, 0),
        K::TYPE_DEF => (ty::TYPE, DECLARATION),
        K::SOURCE_DEF | K::MULTI_SOURCE_DEF | K::MAPPING_HEADER => (ty::VARIABLE, DECLARATION),
        K::SHAPE_EXPR | K::RENAME_ATTR => (ty::TYPE, 0),
        K::RENAME => (ty::PROPERTY, DECLARATION),
        K::ALIAS_ARG if is_last_ident(tok) => (ty::VARIABLE, DECLARATION),
        K::PROPERTY_LHS => (ty::PROPERTY, 0),
        K::NAMED_ARG => (ty::PARAMETER, 0),
        K::POSTFIX_EXPR if follows_sibling(tok, K::DOT) => {
            if is_callee(&parent) {
                (ty::FUNCTION, 0)
            } else {
                (ty::PROPERTY, 0)
            }
        }
        K::LITERAL_EXPR => (bare_name(tok.text(), &parent, names), 0),
        _ => (ty::VARIABLE, 0),
    }
}

/// `type` before a destructuring's braces, and `as` — the middle IDENT of a
/// rename (`"…" as name`) or of an alias (`Node as Other`).
fn is_contextual_keyword(tok: &SyntaxToken, parent: SyntaxKind) -> bool {
    match parent {
        SyntaxKind::TYPE_DEF => !follows_sibling(tok, SyntaxKind::LBRACE),
        SyntaxKind::RENAME => !is_last_ident(tok),
        SyntaxKind::ALIAS_ARG => !is_last_ident(tok) && follows_sibling(tok, SyntaxKind::IDENT),
        _ => false,
    }
}

/// A name in expression position, in the order the lowering reads one:
///
/// 1. the head of a dotted name the CATALOGUE owns (`io` in `io.csv`, `str` in
///    `str.lower`) — a namespace, or a type when the head names one
///    ([`fossil_hir::stdlib::receiver_of`]);
/// 2. a name a `type` binding introduced — `Person(User.email)` is an edge to
///    that shape, which the grammar writes as a call;
/// 3. any other callee — a function;
/// 4. everything else — a variable: a source, a mapping, an alias.
fn bare_name(name: &str, literal: &SyntaxNode, names: &FileNames) -> u32 {
    use fossil_hir::stdlib::{Receiver, receiver_of, stdlib};
    if is_member_receiver(literal) && stdlib().is_catalogued_head(name) {
        return match receiver_of(name) {
            Receiver::Namespace => ty::NAMESPACE,
            Receiver::Scalar(_) | Receiver::Relation => ty::TYPE,
        };
    }
    if names.is_type(name) {
        return ty::TYPE;
    }
    if is_callee(literal) {
        return ty::FUNCTION;
    }
    ty::VARIABLE
}

/// Whether `node` is the callee of a call: the first child of a `POSTFIX_EXPR`
/// that carries a `(`.
fn is_callee(node: &SyntaxNode) -> bool {
    let Some(postfix) = node.parent() else {
        return false;
    };
    postfix.kind() == SyntaxKind::POSTFIX_EXPR
        && postfix.first_child().is_some_and(|c| &c == node)
        && has_token_child(&postfix, SyntaxKind::LPAREN)
}

/// Whether `node` is the left of a `.` — the first child of a `POSTFIX_EXPR`
/// that carries a `DOT`.
fn is_member_receiver(node: &SyntaxNode) -> bool {
    let Some(postfix) = node.parent() else {
        return false;
    };
    postfix.kind() == SyntaxKind::POSTFIX_EXPR
        && postfix.first_child().is_some_and(|c| &c == node)
        && has_token_child(&postfix, SyntaxKind::DOT)
}

fn has_token_child(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .any(|t| t.kind() == kind)
}

/// Whether a sibling token of `kind` comes before `tok` in its parent.
fn follows_sibling(tok: &SyntaxToken, kind: SyntaxKind) -> bool {
    std::iter::successors(
        tok.prev_sibling_or_token(),
        fossil_syntax::SyntaxElement::prev_sibling_or_token,
    )
    .any(|el| el.kind() == kind)
}

/// Whether `tok` is the last `IDENT` among its parent's children.
fn is_last_ident(tok: &SyntaxToken) -> bool {
    !std::iter::successors(
        tok.next_sibling_or_token(),
        fossil_syntax::SyntaxElement::next_sibling_or_token,
    )
    .any(|el| el.kind() == SyntaxKind::IDENT)
}

/// A decoded, absolute-positioned token, before delta-encoding. `line` /
/// `start_char` are UTF-16 coordinates; `length` is a UTF-16 code-unit count.
#[derive(Debug, Clone, Copy)]
struct AbsToken {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

/// One span in LSP coordinates, or `None` when it is empty. A span that runs
/// over a line break (a multi-line string or comment) is recorded on its start
/// line with its first-line length: an LSP token cannot cross a line unless the
/// client declares `multilineTokenSupport`, and Monaco does not.
fn abs_token(index: &LineIndex, text: &str, s: &SemanticSpan) -> Option<AbsToken> {
    let start = index.position(s.range.start);
    let end = index.position(s.range.end);
    let length = if end.line == start.line {
        end.character.saturating_sub(start.character)
    } else {
        let slice = text.get(s.range.start as usize..s.range.end as usize)?;
        let first = slice.split('\n').next().unwrap_or(slice);
        u32::try_from(first.encode_utf16().count()).ok()?
    };
    (length > 0).then_some(AbsToken {
        line: start.line,
        start_char: start.character,
        length,
        token_type: s.token_type,
        modifiers: s.modifiers,
    })
}

/// Delta-encode absolute tokens into the LSP flat `Vec<u32>` 5-tuple stream.
/// `deltaLine` is relative to the previous token's line; `deltaStartChar` is
/// relative to the previous token's start *when on the same line*, else absolute.
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
        data.extend_from_slice(&[delta_line, delta_start, t.length, t.token_type, t.modifiers]);
        prev_line = t.line;
        prev_start = t.start_char;
    }
    data
}

/// Decode a flat 5-tuple stream back into absolute
/// `(line, col, len, type, modifiers)` rows.
///
/// Public so the snapshot test (and a future LSP-side assertion) can produce a
/// stable, human-reviewable view of the token stream.
#[must_use]
pub fn decode_tokens(data: &[u32]) -> Vec<(u32, u32, u32, u32, u32)> {
    let mut out = Vec::new();
    let mut line = 0u32;
    let mut col = 0u32;
    for chunk in data.chunks_exact(5) {
        let (dl, dc, len, ty, mods) = (chunk[0], chunk[1], chunk[2], chunk[3], chunk[4]);
        if dl == 0 {
            col += dc;
        } else {
            line += dl;
            col = dc;
        }
        out.push((line, col, len, ty, mods));
    }
    out
}

/// Human-readable legend type name for a `tokenType` index — the snapshot
/// test renders through it, and `fossil-wasm` sends it across the boundary in
/// place of the number.
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
        ty::PARAMETER => "parameter",
        _ => "unknown",
    }
}

/// Human-readable legend modifier name for a modifier BIT index.
#[must_use]
pub const fn legend_modifier_name(bit: u32) -> &'static str {
    match bit {
        md::DECLARATION => "declaration",
        _ => "unknown",
    }
}

/// The names of every modifier set in `modifiers`, in bit order.
#[must_use]
pub fn modifier_names(modifiers: u32) -> Vec<&'static str> {
    (0..u32::try_from(LEGEND_MODIFIERS.len()).unwrap_or(0))
        .filter(|bit| modifiers & (1 << bit) != 0)
        .map(legend_modifier_name)
        .collect()
}

#[cfg(test)]
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

    /// `(text, type, modifiers)` for every span, so a test names what it
    /// means rather than a column.
    fn spans(src: &str) -> Vec<(String, &'static str, Vec<&'static str>)> {
        let (db, file) = db_file(src);
        semantic_spans(&db, file)
            .into_iter()
            .map(|s| {
                (
                    src[s.range.start as usize..s.range.end as usize].to_string(),
                    legend_type_name(s.token_type),
                    modifier_names(s.modifiers),
                )
            })
            .collect()
    }

    /// The kind of the first span whose text is `text`.
    fn kind_of(
        all: &[(String, &'static str, Vec<&'static str>)],
        text: &str,
    ) -> (&'static str, Vec<&'static str>) {
        all.iter().find(|(t, _, _)| t == text).map_or_else(
            || panic!("no span `{text}` in {all:?}"),
            |(_, k, m)| (*k, m.clone()),
        )
    }

    /// Every span whose text is `text`, by kind.
    fn kinds_of(
        all: &[(String, &'static str, Vec<&'static str>)],
        text: &str,
    ) -> Vec<&'static str> {
        all.iter()
            .filter(|(t, _, _)| t == text)
            .map(|(_, k, _)| *k)
            .collect()
    }

    const SRC: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
Users : Person from users
    @subject = \"https://example.org/u/{users.id}\"
";

    const SHOP: &str = "\
@rename(Person, \"http://x/name\" as foaf_name)
type { Person, Order } := io.shex(\"@MinIO dev/shop.shex\")
User := io.csv(\"@MinIO dev bucket/people.csv\", delimiter = \"|\")
Adults := User.where(User.age >= 18).join(User as Other, on = User.id == Other.id)
Orders : Order from Adults
    @subject = \"@MinIO dev bucket/{User.email}\"
    buyer = Person(User.email)
    label = str.lower(User.name)
    note = \"@MinIO dev bucket/people.csv\"
";

    #[test]
    fn the_legend_declares_declaration_as_its_one_modifier() {
        assert_eq!(
            semantic_legend().token_modifiers,
            vec![SemanticTokenModifier::DECLARATION]
        );
        assert_eq!(modifier_names(DECLARATION), vec!["declaration"]);
        assert!(modifier_names(0).is_empty());
    }

    #[test]
    fn tokens_are_a_multiple_of_five() {
        let (db, file) = db_file(SRC);
        let data = semantic_tokens(&db, file);
        assert_eq!(data.len() % 5, 0, "the token stream must be 5-tuples");
        assert!(!data.is_empty(), "a program should emit tokens");
    }

    /// The shape names are types wherever they are written: declared in the
    /// destructuring, named by a mapping header and a `@rename`, and applied
    /// as an edge — a call the grammar cannot tell from a function's.
    #[test]
    fn a_shape_name_is_a_type_everywhere_it_is_written() {
        let all = spans(SHOP);
        let person = all
            .iter()
            .filter(|(t, _, _)| t == "Person")
            .map(|(_, k, m)| (*k, m.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            person,
            vec![
                ("type", vec![]),              // @rename(Person, …)
                ("type", vec!["declaration"]), // type { Person, … }
                ("type", vec![]),              // Person(User.email)
            ],
            "{all:?}"
        );
        assert_eq!(kinds_of(&all, "Order"), vec!["type", "type"], "{all:?}");
        assert_eq!(kind_of(&all, "Order").1, vec!["declaration"]);
    }

    /// `type` and `as` are contextual: IDENTs to the lexer, keywords by place.
    #[test]
    fn the_contextual_keywords_are_keywords_in_their_place() {
        let all = spans(SHOP);
        assert_eq!(kind_of(&all, "type").0, "keyword");
        assert_eq!(kinds_of(&all, "as"), vec!["keyword", "keyword"], "{all:?}");
        assert_eq!(
            kind_of(&all, "foaf_name"),
            ("property", vec!["declaration"])
        );
        assert_eq!(kind_of(&all, "Other"), ("variable", vec!["declaration"]));
    }

    #[test]
    fn a_binding_name_is_a_declaration() {
        let all = spans(SHOP);
        assert_eq!(kind_of(&all, "User"), ("variable", vec!["declaration"]));
        assert_eq!(kind_of(&all, "Adults"), ("variable", vec!["declaration"]));
        assert_eq!(kind_of(&all, "Orders"), ("variable", vec!["declaration"]));
        // A use is a variable and not a declaration.
        let uses = all
            .iter()
            .filter(|(t, _, _)| t == "User")
            .skip(1)
            .map(|(_, k, m)| (*k, m.is_empty()))
            .collect::<Vec<_>>();
        assert!(uses.iter().all(|&u| u == ("variable", true)), "{uses:?}");
    }

    /// A member is a column unless it is called: `io.csv(…)` and
    /// `User.where(…)` are functions, `User.age` is a property.
    #[test]
    fn a_called_member_is_a_function_and_an_uncalled_one_a_property() {
        let all = spans(SHOP);
        for f in ["shex", "csv", "where", "join", "lower"] {
            assert_eq!(kind_of(&all, f).0, "function", "`{f}`: {all:?}");
        }
        for p in ["age", "email", "name"] {
            assert_eq!(kind_of(&all, p).0, "property", "`{p}`: {all:?}");
        }
    }

    /// A quoted member, `Knows."Person.id"`, is a field's name and is coloured
    /// as the bare one is; a string anywhere else stays a string.
    #[test]
    fn a_quoted_member_is_a_property_and_a_string_literal_is_not() {
        let all = spans(
            "K := io.csv(\"k.csv\")\nF : Person from K\n    @subject = \"u/{K.id}\"\n    \
             note = K.\"Person.id\" == \"Person.id\" ? \"a\" : \"b\"\n",
        );
        assert_eq!(
            kinds_of(&all, "\"Person.id\""),
            vec!["property", "string"],
            "{all:?}"
        );
    }

    /// `io` is a namespace the catalogue owns; `str` names a type.
    #[test]
    fn a_catalogued_head_is_a_namespace_or_a_type() {
        let all = spans(SHOP);
        assert_eq!(kinds_of(&all, "io"), vec!["namespace", "namespace"]);
        assert_eq!(kind_of(&all, "str").0, "type");
    }

    #[test]
    fn a_body_key_is_a_property_and_a_named_argument_a_parameter() {
        let all = spans(SHOP);
        assert_eq!(kind_of(&all, "buyer").0, "property");
        assert_eq!(kind_of(&all, "label").0, "property");
        assert_eq!(kind_of(&all, "delimiter").0, "parameter");
        assert_eq!(kind_of(&all, "on").0, "parameter");
    }

    /// The `@name` of a reference is a namespace carved out of its literal,
    /// split where every other reader of a reference splits it — and only in a
    /// reference: the same text as a body value names no connection.
    #[test]
    fn a_connection_is_carved_out_of_the_reference_that_names_it() {
        let all = spans(SHOP);
        let carved = |name: &str| {
            let i = all
                .iter()
                .position(|(t, _, _)| t == name)
                .unwrap_or_else(|| panic!("no `{name}` in {all:?}"));
            all[i - 1..=i + 1]
                .iter()
                .map(|(t, k, _)| (t.as_str(), *k))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            carved("@MinIO dev bucket"),
            vec![
                ("\"", "string"),
                ("@MinIO dev bucket", "namespace"),
                ("/people.csv\"", "string"),
            ]
        );
        assert_eq!(carved("@MinIO dev")[2], ("/shop.shex\"", "string"));
        assert_eq!(
            all.iter().filter(|(_, k, _)| *k == "namespace").count(),
            4,
            "two connections and two `io` heads, and nothing from the body: {all:?}"
        );
    }

    #[test]
    fn a_reference_without_a_connection_stays_one_string() {
        let all = spans(SRC);
        assert_eq!(kind_of(&all, "\"users.csv\"").0, "string");
    }

    /// The hole closes in the colour it opened in, and the destructuring's
    /// brace on line 0 is not mistaken for it: line 0 keeps its one operator.
    #[test]
    fn the_interpolation_closes_in_the_colour_it_opened() {
        let (db, file) = db_file(SRC);
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        let ops_on = |want: u32| -> Vec<u32> {
            decoded
                .iter()
                .filter(|&&(line, _, _, t, _)| line == want && t == ty::OPERATOR)
                .map(|&(_, col, _, _, _)| col)
                .collect()
        };
        assert_eq!(ops_on(3), vec![13, 38, 47], "all tokens: {decoded:?}");
        assert_eq!(ops_on(0), vec![16], "all tokens: {decoded:?}");
    }

    #[test]
    fn the_declaration_bit_reaches_the_wire() {
        let (db, file) = db_file(SRC);
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        let users = decoded
            .iter()
            .find(|&&(line, col, _, _, _)| line == 1 && col == 0)
            .expect("`users` at 1:0");
        assert_eq!((users.3, users.4), (ty::VARIABLE, DECLARATION));
    }

    #[test]
    fn comment_classified_as_comment() {
        // Fossil comments are `//`-to-EOL (lexer.rs), NOT `#`.
        let (db, file) = db_file(&format!("// a comment\n{SRC}"));
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        assert!(
            decoded.iter().any(|&(_, _, _, t, _)| t == ty::COMMENT),
            "expected a comment token: {decoded:?}"
        );
    }

    #[test]
    fn delta_encoding_is_relative() {
        let abs = [
            AbsToken {
                line: 0,
                start_char: 0,
                length: 6,
                token_type: ty::KEYWORD,
                modifiers: 0,
            },
            AbsToken {
                line: 0,
                start_char: 7,
                length: 2,
                token_type: ty::NAMESPACE,
                modifiers: DECLARATION,
            },
        ];
        let data = delta_encode(&abs);
        assert_eq!(&data[5..10], &[0, 7, 2, ty::NAMESPACE, DECLARATION]);
    }

    #[test]
    fn utf16_columns_for_multibyte_comment() {
        // Read as bytes, `café` is five and the next line's column would be off.
        let (db, file) = db_file(&format!("// café\n{SRC}"));
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        let first_on_line_1 = decoded
            .iter()
            .find(|&&(line, _, _, _, _)| line == 1)
            .expect("a token on the line after the comment");
        assert_eq!(first_on_line_1.1, 0);
    }
}
