//! `SyntaxKind` — the kind tag for every node and token in the Fossil CST,
//! plus the `rowan::Language` impl that wires it into the green/red tree.
//!
//! Every variant here is a kind the lexer or the parser actually produces —
//! which is NOT the same as a terminal of `grammar.bnf`. None of them names a
//! spelling the grammar does not have, and the grammar has no terminal without
//! a kind here. The parser is what this enum describes.
//!
//! The `repr(u16)` values are an implementation detail of the rowan green
//! tree and are NOT a compatibility surface: nothing persists a raw value
//! across a build, so adding or removing a variant renumbers the rest.

/// Declare [`SyntaxKind`] and the slice of every variant in declaration
/// order, from one list — so a raw value maps back by indexing, and no second
/// list of the variants exists to fall out of step.
macro_rules! syntax_kinds {
    ($($(#[$attr:meta])* $kind:ident,)*) => {
        // SCREAMING_SNAKE_CASE is the rust-analyzer / rowan-ecosystem convention
        // for SyntaxKind variants, and it is the naming `grammar.bnf` uses for
        // its terminals.
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        pub enum SyntaxKind {
            $($(#[$attr])* $kind,)*
        }

        impl SyntaxKind {
            const ALL: &[Self] = &[$(Self::$kind,)*];
        }
    };
}

syntax_kinds! {
    // ─── Trivia ───────────────────────────────────────────────────────
    WHITESPACE,
    NEWLINE,
    COMMENT,

    // ─── Lexical tokens ───────────────────────────────────────────────
    IDENT,
    INTEGER,
    FLOAT,
    /// `true` / `false` — `BOOL := 'true' | 'false'` (grammar.bnf, BOOL).
    ///
    /// ONE kind for both spellings: which one it is, is the token's text, and a
    /// pair of kinds would make every consumer match twice to learn one bit.
    BOOL,
    /// `null` — the absence of a value, `NULL := 'null'` (grammar.bnf).
    NULL,
    STRING,
    /// `"birth date"@en` — `LANG_STRING := STRING LANGTAG` (grammar.bnf,
    /// LANG_STRING): a constant `rdf:langString`, the tag touching the quote.
    LANG_STRING,
    /// The opening `"` of an interpolated string.
    ///
    /// A string with no interpolation stays one `STRING` token: the carve
    /// happens only where there is something to carve.
    STRING_OPEN,
    /// A literal run between two interpolations, or between a delimiter and
    /// one. Carries its source text verbatim, `{{` included.
    STRING_TEXT,
    /// The opening of a hole — `{`. There is one spelling.
    INTERP_OPEN,
    /// The closing `"`. The hole's own `}` is an ordinary `RBRACE`.
    STRING_CLOSE,
    AT_ATTR,

    // ─── Punctuation / operators ──────────────────────────────────────
    DEFINE,
    ASSIGN,
    SHAPE_SEP,
    DOT,
    COMMA,
    LPAREN,
    RPAREN,
    LBRACE,
    RBRACE,
    // No `PIPE`: `|>` would be a second spelling of `a.f()`. `|` matches no
    // lexer rule, so the bytes reach the parser as an ERROR token and are
    // refused by name.
    EQ,
    NEQ,
    LT,
    LE,
    GT,
    GE,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    PERCENT,
    T_QUESTION,

    // ─── Keywords ─────────────────────────────────────────────────────
    // The `KW_*` half of grammar.bnf, § RESERVED KEYWORDS. The rest of that
    // list — `true`, `false`, `null` — are LITERALS, reserved by being tokens,
    // and arrive as [`SyntaxKind::BOOL`] / [`SyntaxKind::NULL`].
    KW_FROM,
    KW_AND,
    KW_OR,
    KW_NOT,

    // ─── Virtual (post-lexer) ─────────────────────────────────────────
    INDENT,
    DEDENT,

    // ─── Composite item nodes ─────────────────────────────────────────
    PROGRAM,
    // No vocabulary declaration: a shape name, a property key, an expression and
    // an interpolation hole are bare names or full IRIs in strings, so a prefix
    // nothing spells would be a statement about nothing.
    SOURCE_DEF,
    /// `{ A, B, ... } := io.rdf(uri, schema = shex)` — a destructuring source
    /// definition binding N members (one per declared shape) to a single source.
    MULTI_SOURCE_DEF,
    /// `type { Place as City } := io.shex("places.shex")` — a type import. The
    /// same member list as `MULTI_SOURCE_DEF`, in type position, one
    /// [`SyntaxKind::IMPORT_MEMBER`] per name (grammar.bnf, `TypeDef`).
    ///
    /// The binder is `:=` — one binder, and what is being bound is read off the
    /// left-hand side (grammar.bnf, DEFINE). Every `=` in the
    /// grammar is an ASSIGNMENT: a body property, `@subject`, a named argument.
    TYPE_DEF,
    /// `Place` or `Place as City` — `ImportMember := IDENT ('as' IDENT)?`
    /// (grammar.bnf, `ImportMember`), inside a `TYPE_DEF` or a
    /// `MULTI_SOURCE_DEF`.
    ///
    /// The first `IDENT` is the LOCAL NAME of a shape the document declares;
    /// the one after `as`, when there is one, is the name the program uses.
    /// `as` is contextual here as in [`SyntaxKind::ALIAS_ARG`]: an `IDENT`
    /// followed by a bare `IDENT`, which nothing else in a member list can be.
    IMPORT_MEMBER,
    /// `@iri("…") type Person` and its indented fields — `TypeDecl :=
    /// DeclAttrs 'type' IDENT NEWLINE INDENT Field+ DEDENT` (grammar.bnf,
    /// `TypeDecl`).
    ///
    /// An output type written in the program. Its attributes — `IRI_ATTR` and
    /// any `ANNOTATION_ATTR` — are children, so a declaration whose IRI is
    /// missing still has a node to hang the refusal on, and the IRI travels
    /// with the type it names.
    TYPE_DECL,
    /// `@iri("https://schema.org/name") name: String` — `Field := DeclAttrs
    /// IDENT SHAPE_SEP TypeExpr NEWLINE` (grammar.bnf, `Field`). Its attributes
    /// are on its line or stacked on the lines above it.
    FIELD,
    /// `@iri("…")` — `IriAttr := AT_ATTR LPAREN STRING RPAREN` (grammar.bnf,
    /// `IriAttr`), above a `TYPE_DECL` and at the start of each `FIELD`.
    ///
    /// It shares its `AT_ATTR` token with `@subject` and POSITION tells them
    /// apart (disambiguation rule 6; grammar.bnf, § DISAMBIGUATION RULES).
    IRI_ATTR,
    /// `@annotation("…rdf-schema#label", "birth date"@en)` — `AnnotationAttr
    /// := AT_ATTR LPAREN STRING COMMA AnnotationObject RPAREN` (grammar.bnf,
    /// `AnnotationAttr`): a ShEx annotation, a predicate and an object, above a
    /// `TYPE_DECL` or a `FIELD`, any number of times.
    ANNOTATION_ATTR,
    /// `String`, `Option<Date>`, `Seq<Ref<City>>` — `TypeExpr := IDENT (LT
    /// TypeExpr GT)?` (grammar.bnf, `TypeExpr`). The argument is a nested
    /// `TYPE_EXPR`; which names take one is the checker's.
    TYPE_EXPR,
    MAPPING,
    MAPPING_HEADER,
    MAPPING_BODY,
    PROPERTY,
    PROPERTY_LHS,
    /// The mapping header's shape (grammar.bnf, `ShapeExpr`) — `ShapeExpr :=
    /// IDENT`. One `IDENT` token child and nothing else: one of the names a
    /// `type { … } := …` binding introduced. There is no `&` intersection: a
    /// mapping produces one shape.
    SHAPE_EXPR,

    // ─── Composite expression nodes ───────────────────────────────────
    EXPR,
    /// A string with at least one hole — `STRING_OPEN (STRING_TEXT |
    /// INTERPOLATION)* STRING_CLOSE`. One spelling, one node.
    INTERP_STRING_EXPR,
    /// One hole — `INTERP_OPEN Expression RBRACE`. The expression is an
    /// ordinary expression, parsed by the ordinary expression parser: there is
    /// no format mini-language to keep in step with the checker.
    INTERPOLATION,
    /// A one-token primary — a number, a string, `true`/`false`, `null` or a
    /// name. A constant IRI is a STRING, and the shape decides that it denotes
    /// rather than reads.
    LITERAL_EXPR,
    TERNARY_EXPR,
    BINARY_EXPR,
    UNARY_EXPR,
    POSTFIX_EXPR,
    PAREN_EXPR,
    ARG_LIST,
    ARG,
    NAMED_ARG,
    /// `Node as Other` — `AliasArg := IDENT 'as' IDENT` (grammar.bnf, `AliasArg`).
    ///
    /// The self-join's alias, and the OTHER place `as` survives:
    /// `Node.join(Node as Other, on = Node.parent == Other.id)` binds a second
    /// name for the same source so the two sides can be told apart, and the
    /// mapping body then writes `Other.label` next to `Node.label`.
    ///
    /// Same one-token lookahead as [`SyntaxKind::IMPORT_MEMBER`]: an operand
    /// followed by a bare `IDENT`, which no expression can continue.
    ALIAS_ARG,

    // ─── Error / sentinel ─────────────────────────────────────────────
    ERROR,
    EOF,

    /// Sentinel — must be the last variant. Used for round-trip bounds checks.
    #[doc(hidden)]
    __LAST,
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(k: SyntaxKind) -> Self {
        Self(k as u16)
    }
}

impl SyntaxKind {
    /// Reverse map from a raw `u16` (as stored by `rowan`) back to the typed enum.
    fn from_raw_value(v: u16) -> Self {
        Self::ALL[usize::from(v)]
    }
}

/// Marker enum implementing [`rowan::Language`] for the Fossil CST.
///
/// `enum {}` (uninhabited) — never instantiated; only used at the type level.
/// `rowan::Language` requires `Sized + Copy + Debug + Eq + Ord + Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FossilLang {}

impl rowan::Language for FossilLang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        assert!(
            raw.0 < SyntaxKind::__LAST as u16,
            "rowan SyntaxKind out of range: {}",
            raw.0
        );
        SyntaxKind::from_raw_value(raw.0)
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<FossilLang>;
pub type SyntaxToken = rowan::SyntaxToken<FossilLang>;
pub type SyntaxElement = rowan::SyntaxElement<FossilLang>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rowan_language_round_trips_every_kind() {
        use rowan::Language;
        for raw in 0..(SyntaxKind::__LAST as u16) {
            let kind = FossilLang::kind_from_raw(rowan::SyntaxKind(raw));
            let back = FossilLang::kind_to_raw(kind);
            assert_eq!(back.0, raw);
        }
    }
}
