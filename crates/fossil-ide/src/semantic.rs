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
//! token *types* and *modifiers*. Tokens themselves are `lsp_types::SemanticToken`s,
//! delta-encoded relative to the previous token (the LSP wire format). The
//! `tokenType` is an *index* into the legend's `token_types`; `length` and
//! `deltaStartChar` are **UTF-16 code units** (Monaco counts UTF-16), so we route
//! every column / length through the [`crate::position::LineIndex`]
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
use lsp_types::{
    Position, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend,
};
use rowan::WalkEvent;
use smol_str::SmolStr;

use crate::position::{LineIndex, line_index, position};

/// Declares [`TokenType`] and the legend's `token_types` from ONE list, so a
/// variant's discriminant is its index in the legend by construction. A new
/// type is APPENDED: a client that cached the legend reads every index before
/// it unchanged.
macro_rules! token_types {
    ($($variant:ident => $lsp:ident),* $(,)?) => {
        /// A legend token type. Its discriminant IS the `tokenType` of the LSP
        /// wire, an index into [`semantic_legend`]'s `token_types`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u32)]
        pub enum TokenType { $($variant),* }

        static LEGEND_TYPES: &[SemanticTokenType] = &[$(SemanticTokenType::$lsp),*];
    };
}

token_types! {
    Keyword => KEYWORD,
    Namespace => NAMESPACE,
    Type => TYPE,
    Function => FUNCTION,
    Property => PROPERTY,
    String => STRING,
    Number => NUMBER,
    Operator => OPERATOR,
    Comment => COMMENT,
    Variable => VARIABLE,
    Parameter => PARAMETER,
}

impl TokenType {
    /// The LSP name of this type — `"keyword"`, `"namespace"` — read off the
    /// legend entry it indexes.
    #[must_use]
    pub fn name(self) -> &'static str {
        LEGEND_TYPES[self as usize].as_str()
    }
}

/// The legend's token *modifiers*, in bit order: modifier `i` is bit `1 << i`
/// of the emitted `tokenModifiers`.
static LEGEND_MODIFIERS: &[SemanticTokenModifier] = &[SemanticTokenModifier::DECLARATION];

/// The bitset for a token carrying `declaration`.
const DECLARATION: u32 = 1 << 0;

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
    /// The legend type it is painted as.
    pub token_type: TokenType,
    /// Bitset over the legend's `token_modifiers` — [`modifier_names`] reads it.
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
            spans.push(span(range.start..at, TokenType::String, 0));
            spans.push(span(alias, TokenType::Namespace, 0));
            spans.push(span(past..range.end, TokenType::String, 0));
        } else if let Some((token_type, modifiers)) = classify(&tok, &names) {
            spans.push(span(byte_range(&tok), token_type, modifiers));
        }
    }
    spans
}

/// Full-document semantic tokens for `file` — exactly what
/// `textDocument/semanticTokens/full` returns: [`semantic_spans`] through the
/// FILE-keyed [`LineIndex`], each token's line relative to the previous one and
/// its start relative to the previous one on the same line.
#[must_use]
pub fn semantic_tokens(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<SemanticToken> {
    let index = line_index(db, file);
    let text = file.text(db);
    let mut prev = Position::default();
    semantic_spans(db, file)
        .iter()
        .filter_map(|s| {
            let (at, length) = lsp_span(index, text, s)?;
            let delta_start = if at.line == prev.line {
                at.character - prev.character
            } else {
                at.character
            };
            let token = SemanticToken {
                delta_line: at.line - prev.line,
                delta_start,
                length,
                token_type: s.token_type as u32,
                token_modifiers_bitset: s.modifiers,
            };
            prev = at;
            Some(token)
        })
        .collect()
}

/// What the file itself declares, read once per walk.
struct FileNames {
    /// The names `type { … } := …` introduced — a call to one of them is an
    /// edge to that shape, not a function.
    types: Vec<SmolStr>,
    /// Every string a binding reads as a reference: a source's URI, its
    /// `schema =` document, a `type` binding's document. These are the strings
    /// `fossil-lineage` reports and the location resolves, so they are the only
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

const fn span(range: Range<u32>, token_type: TokenType, modifiers: u32) -> SemanticSpan {
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
/// The split is [`fossil_location::split_alias`]'s, the rule every other reader
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
    let (alias, _) = fossil_location::split_alias(inner)?;
    let at = u32::from(tok.text_range().start()) + 1;
    let len = u32::try_from(alias.len() + 1).ok()?;
    Some(at..at + len)
}

/// Classify a leaf [`SyntaxToken`] into a legend `(type, modifiers)`, or `None`
/// if it carries no color (whitespace, structural punctuation, indent/dedent,
/// errors). An `IDENT` is disambiguated by its place in the tree — see
/// [`ident`].
fn classify(tok: &SyntaxToken, names: &FileNames) -> Option<(TokenType, u32)> {
    use SyntaxKind as K;
    let token_type = match tok.kind() {
        // ── unambiguous lexical classes ───────────────────────────────
        K::COMMENT => TokenType::Comment,
        // A string with a hole is carved into a run of tokens, so every part
        // of it has to be named here or the literal loses its colour halfway
        // through. A STRING after a member's `.` is not a literal: it is a
        // quoted member, `Knows."Person.id"`, coloured as the bare one is.
        K::STRING
            if tok.parent().is_some_and(|p| p.kind() == K::POSTFIX_EXPR)
                && follows_sibling(tok, K::DOT) =>
        {
            TokenType::Property
        }
        K::STRING | K::STRING_OPEN | K::STRING_TEXT | K::STRING_CLOSE => TokenType::String,
        K::INTEGER | K::FLOAT => TokenType::Number,

        // ── the reserved words and the `@attr` marker ─────────────────
        //
        // `from`, `and`, `or`, `not` (grammar.bnf, § RESERVED KEYWORDS). The
        // contextual ones — `type` before a destructuring, `as` in a rename or
        // an alias — are IDENTs and are named by [`ident`] from their place.
        K::KW_FROM | K::KW_AND | K::KW_OR | K::KW_NOT | K::AT_ATTR => TokenType::Keyword,

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
        | K::INTERP_OPEN => TokenType::Operator,

        K::IDENT => return Some(ident(tok, names)),

        // The hole's CLOSER. `}` is an ordinary `RBRACE` shared with
        // `type { Person } := …` and `{ A, B } := …`, and the grammar puts the
        // interpolation's directly under `INTERPOLATION` and nowhere else.
        K::RBRACE if tok.parent().is_some_and(|p| p.kind() == K::INTERPOLATION) => {
            TokenType::Operator
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
fn ident(tok: &SyntaxToken, names: &FileNames) -> (TokenType, u32) {
    use SyntaxKind as K;
    let Some(parent) = tok.parent() else {
        return (TokenType::Variable, 0);
    };
    match parent.kind() {
        _ if is_contextual_keyword(tok, parent.kind()) => (TokenType::Keyword, 0),
        K::TYPE_DEF => (TokenType::Type, DECLARATION),
        K::SOURCE_DEF | K::MULTI_SOURCE_DEF | K::MAPPING_HEADER => {
            (TokenType::Variable, DECLARATION)
        }
        K::SHAPE_EXPR | K::RENAME_ATTR => (TokenType::Type, 0),
        K::RENAME => (TokenType::Property, DECLARATION),
        K::ALIAS_ARG if is_last_ident(tok) => (TokenType::Variable, DECLARATION),
        K::PROPERTY_LHS => (TokenType::Property, 0),
        K::NAMED_ARG => (TokenType::Parameter, 0),
        K::POSTFIX_EXPR if follows_sibling(tok, K::DOT) => {
            if is_callee(&parent) {
                (TokenType::Function, 0)
            } else {
                (TokenType::Property, 0)
            }
        }
        K::LITERAL_EXPR => (bare_name(tok.text(), &parent, names), 0),
        _ => (TokenType::Variable, 0),
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
fn bare_name(name: &str, literal: &SyntaxNode, names: &FileNames) -> TokenType {
    use fossil_hir::stdlib::{Receiver, receiver_of, stdlib};
    if is_member_receiver(literal) && stdlib().is_catalogued_head(name) {
        return match receiver_of(name) {
            Receiver::Namespace => TokenType::Namespace,
            Receiver::Scalar(_) | Receiver::Relation => TokenType::Type,
        };
    }
    if names.is_type(name) {
        return TokenType::Type;
    }
    if is_callee(literal) {
        return TokenType::Function;
    }
    TokenType::Variable
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

/// One span's start in LSP coordinates and its length in UTF-16 code units, or
/// `None` when it is empty. A span that runs over a line break (a multi-line
/// string or comment) is recorded on its start line with its first-line length:
/// an LSP token cannot cross a line unless the client declares
/// `multilineTokenSupport`, and Monaco does not.
fn lsp_span(index: &LineIndex, text: &str, s: &SemanticSpan) -> Option<(Position, u32)> {
    let start = position(index, s.range.start);
    let end = position(index, s.range.end);
    let length = if end.line == start.line {
        end.character.saturating_sub(start.character)
    } else {
        let slice = text.get(s.range.start as usize..s.range.end as usize)?;
        let first = slice.split('\n').next().unwrap_or(slice);
        u32::try_from(first.encode_utf16().count()).ok()?
    };
    (length > 0).then_some((start, length))
}

/// Decode a token stream back into absolute
/// `(line, col, len, type, modifiers)` rows — a stable, human-reviewable view
/// of what [`semantic_tokens`] put on the wire.
#[must_use]
pub fn decode_tokens(data: &[SemanticToken]) -> Vec<(u32, u32, u32, u32, u32)> {
    let mut line = 0u32;
    let mut col = 0u32;
    data.iter()
        .map(|t| {
            if t.delta_line == 0 {
                col += t.delta_start;
            } else {
                line += t.delta_line;
                col = t.delta_start;
            }
            (line, col, t.length, t.token_type, t.token_modifiers_bitset)
        })
        .collect()
}

/// The names of every modifier set in `modifiers`, in bit order.
#[must_use]
pub fn modifier_names(modifiers: u32) -> Vec<&'static str> {
    LEGEND_MODIFIERS
        .iter()
        .enumerate()
        .filter(|(bit, _)| modifiers & (1 << bit) != 0)
        .map(|(_, m)| m.as_str())
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
                    s.token_type.name(),
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
                .filter(|&&(line, _, _, t, _)| line == want && t == TokenType::Operator as u32)
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
        assert_eq!(
            (users.3, users.4),
            (TokenType::Variable as u32, DECLARATION)
        );
    }

    #[test]
    fn comment_classified_as_comment() {
        // Fossil comments are `//`-to-EOL (lexer.rs), NOT `#`.
        let (db, file) = db_file(&format!("// a comment\n{SRC}"));
        let decoded = decode_tokens(&semantic_tokens(&db, file));
        assert!(
            decoded
                .iter()
                .any(|&(_, _, _, t, _)| t == TokenType::Comment as u32),
            "expected a comment token: {decoded:?}"
        );
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
