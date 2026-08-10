//! `SyntaxKind` — the kind tag for every node and token in the Fossil CST,
//! plus the `rowan::Language` impl that wires it into the green/red tree.
//!
//! Every variant here is a kind the lexer or the parser actually produces.
//! The `repr(u16)` values are an implementation detail of the rowan green
//! tree and are NOT a compatibility surface: nothing persists a raw value
//! across a build, so adding or removing a variant renumbers the rest, and
//! the only thing that has to move in lockstep is [`SyntaxKind::from_raw_value`]
//! (guarded by `syntax_kind_round_trip_for_all_variants`).
//!
//! The one cross-language pin is on the LEXER's `Token` discriminants, not on
//! these — see `packages/codemirror-fossil/src/tags.ts`.

// SCREAMING_SNAKE_CASE is the rust-analyzer / rowan-ecosystem convention for
// SyntaxKind variants (matches the BNF terminal naming in `grammar.bnf`).
// Suppress the rustc style warning crate-wide for this enum only.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // ─── Trivia ───────────────────────────────────────────────────────
    WHITESPACE = 0,
    NEWLINE,
    COMMENT,

    // ─── Lexical tokens ───────────────────────────────────────────────
    IDENT,
    INTEGER,
    FLOAT,
    STRING,
    TEMPLATE,
    ABS_IRI,
    ENV_VAR,
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
    PIPE,
    TRIPLE_OPEN,
    TRIPLE_CLOSE,
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
    SHAPE_AND,

    // ─── Keywords ─────────────────────────────────────────────────────
    KW_PREFIX,
    KW_FROM,
    KW_IN,
    KW_USE,
    KW_AS,
    KW_AND,
    KW_OR,
    KW_NOT,
    KW_IRI,

    // ─── Virtual (post-lexer) ─────────────────────────────────────────
    INDENT,
    DEDENT,

    // ─── Composite item nodes ─────────────────────────────────────────
    PROGRAM,
    PREFIX_DECL,
    SOURCE_DEF,
    /// `{ A, B, ... } := io.rdf(uri, schema = shex)` — a destructuring source
    /// definition binding N members (one per declared shape) to a single source.
    MULTI_SOURCE_DEF,
    /// `type { A, B } = io.shex("s.shex")` — a type binding. Same destructuring
    /// as `MULTI_SOURCE_DEF`, in type position: one catalogue, two binders
    /// (ADR-0057, seventh amendment). Binding is positional (tenth).
    TYPE_DEF,
    MAPPING,
    MAPPING_HEADER,
    MAPPING_BODY,
    PROPERTY,
    PROPERTY_LHS,
    IMPORT,
    IMPORT_PATH,
    SELECTIVE_IMPORT,
    ALIAS,
    SHAPE_EXPR,
    IN_CLAUSE,
    ANNOTATION_BLOCK,
    ANNOTATION_ITEM,

    // ─── Composite expression nodes ───────────────────────────────────
    EXPR,
    TEMPLATE_EXPR,
    IRI_EXPR,
    LITERAL_EXPR,
    FIELD_REF_EXPR,
    PIPELINE_EXPR,
    TERNARY_EXPR,
    BINARY_EXPR,
    UNARY_EXPR,
    POSTFIX_EXPR,
    PAREN_EXPR,
    RECORD_LITERAL,
    RECORD_FIELD,
    TRIPLE_TERM,
    ARG_LIST,
    ARG,
    NAMED_ARG,

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
    ///
    /// Implemented as an explicit `match` rather than `unsafe { transmute }` so
    /// the workspace `unsafe_code = "deny"` lint stays clean here. ADR-0004
    /// permits `#[allow(unsafe_code)]` only at third-party-trait integration
    /// boundaries; a value-to-enum decode is not such a boundary.
    ///
    /// The match arms MUST stay in lock-step with the enum declaration order;
    /// the `syntax_kind_round_trip_for_all_variants` unit test guards this.
    fn from_raw_value(v: u16) -> Self {
        match v {
            0 => Self::WHITESPACE,
            1 => Self::NEWLINE,
            2 => Self::COMMENT,
            3 => Self::IDENT,
            4 => Self::INTEGER,
            5 => Self::FLOAT,
            6 => Self::STRING,
            7 => Self::TEMPLATE,
            8 => Self::ABS_IRI,
            9 => Self::ENV_VAR,
            10 => Self::AT_ATTR,
            11 => Self::DEFINE,
            12 => Self::ASSIGN,
            13 => Self::SHAPE_SEP,
            14 => Self::DOT,
            15 => Self::COMMA,
            16 => Self::LPAREN,
            17 => Self::RPAREN,
            18 => Self::LBRACE,
            19 => Self::RBRACE,
            20 => Self::PIPE,
            21 => Self::TRIPLE_OPEN,
            22 => Self::TRIPLE_CLOSE,
            23 => Self::EQ,
            24 => Self::NEQ,
            25 => Self::LT,
            26 => Self::LE,
            27 => Self::GT,
            28 => Self::GE,
            29 => Self::PLUS,
            30 => Self::MINUS,
            31 => Self::STAR,
            32 => Self::SLASH,
            33 => Self::PERCENT,
            34 => Self::T_QUESTION,
            35 => Self::SHAPE_AND,
            36 => Self::KW_PREFIX,
            37 => Self::KW_FROM,
            38 => Self::KW_IN,
            39 => Self::KW_USE,
            40 => Self::KW_AS,
            41 => Self::KW_AND,
            42 => Self::KW_OR,
            43 => Self::KW_NOT,
            44 => Self::KW_IRI,
            45 => Self::INDENT,
            46 => Self::DEDENT,
            47 => Self::PROGRAM,
            48 => Self::PREFIX_DECL,
            49 => Self::SOURCE_DEF,
            50 => Self::MULTI_SOURCE_DEF,
            51 => Self::TYPE_DEF,
            52 => Self::MAPPING,
            53 => Self::MAPPING_HEADER,
            54 => Self::MAPPING_BODY,
            55 => Self::PROPERTY,
            56 => Self::PROPERTY_LHS,
            57 => Self::IMPORT,
            58 => Self::IMPORT_PATH,
            59 => Self::SELECTIVE_IMPORT,
            60 => Self::ALIAS,
            61 => Self::SHAPE_EXPR,
            62 => Self::IN_CLAUSE,
            63 => Self::ANNOTATION_BLOCK,
            64 => Self::ANNOTATION_ITEM,
            65 => Self::EXPR,
            66 => Self::TEMPLATE_EXPR,
            67 => Self::IRI_EXPR,
            68 => Self::LITERAL_EXPR,
            69 => Self::FIELD_REF_EXPR,
            70 => Self::PIPELINE_EXPR,
            71 => Self::TERNARY_EXPR,
            72 => Self::BINARY_EXPR,
            73 => Self::UNARY_EXPR,
            74 => Self::POSTFIX_EXPR,
            75 => Self::PAREN_EXPR,
            76 => Self::RECORD_LITERAL,
            77 => Self::RECORD_FIELD,
            78 => Self::TRIPLE_TERM,
            79 => Self::ARG_LIST,
            80 => Self::ARG,
            81 => Self::NAMED_ARG,
            82 => Self::ERROR,
            83 => Self::EOF,
            _ => panic!("invalid SyntaxKind raw value: {v}"),
        }
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
    fn syntax_kind_round_trip_for_all_variants() {
        for raw in 0..(SyntaxKind::__LAST as u16) {
            let kind = SyntaxKind::from_raw_value(raw);
            assert_eq!(kind as u16, raw, "round-trip failed at raw={raw}");
        }
    }

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
