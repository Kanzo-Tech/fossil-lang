//! `SyntaxKind` — the kind tag for every node and token in the Fossil CST,
//! plus the `rowan::Language` impl that wires it into the green/red tree.
//!
//! Phase 1 carried the subset of grammar.bnf needed for `examples/hello.fossil`:
//! prefix decls, source defs, mappings (header + body of properties).
//! Phase 2 adds the full operator/expression/annotation token surface plus
//! composite node kinds for the Pratt expression sub-parser and the full
//! item parser. Phase 1 numeric IDs are preserved (existing variants keep
//! their `repr(u16)` values) so any callers that cached raw values do not
//! break; Phase 2 variants are APPENDED before `__LAST`.

// SCREAMING_SNAKE_CASE is the rust-analyzer / rowan-ecosystem convention for
// SyntaxKind variants (matches the BNF terminal naming in `grammar.bnf`).
// Suppress the rustc style warning crate-wide for this enum only.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // ─── Phase 1 variants (IDs frozen) ────────────────────────────────
    // Trivia
    WHITESPACE = 0,
    NEWLINE,
    COMMENT,

    // Lexical tokens (Phase 1 subset)
    IDENT,
    INTEGER,
    STRING,
    TEMPLATE,
    ABS_IRI,
    PREFIXED_NAME,
    FIELD_REF,

    // Punctuation
    DEFINE,
    ASSIGN,
    SHAPE_SEP,
    DOT,
    COMMA,
    LPAREN,
    RPAREN,
    LBRACE,
    RBRACE,
    LANGLE,
    RANGLE,

    // Keywords
    KW_PREFIX,
    KW_FROM,

    // Virtual (post-lexer)
    INDENT,
    DEDENT,

    // Composite nodes (Phase 1)
    PROGRAM,
    PREFIX_DECL,
    SOURCE_DEF,
    MAPPING,
    MAPPING_HEADER,
    MAPPING_BODY,
    PROPERTY,
    PROPERTY_LHS,
    EXPR,
    CALL_EXPR,
    TEMPLATE_EXPR,
    IRI_EXPR,
    LITERAL_EXPR,
    FIELD_REF_EXPR,

    // Error / sentinel
    ERROR,
    EOF,

    // ─── Phase 2: lexical tokens ──────────────────────────────────────
    FLOAT,
    ENV_VAR,
    PARTIAL,
    AT_EXPORT,
    AT_ATTR,

    // ─── Phase 2: punctuation / operators ─────────────────────────────
    PIPE,
    ARROW,
    TYPE_ANNOT,
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

    // ─── Phase 2: keywords ────────────────────────────────────────────
    KW_IN,
    KW_USE,
    KW_AS,
    KW_AND,
    KW_OR,
    KW_NOT,
    KW_IRI,

    // ─── Phase 2: composite expression nodes (Pratt-built) ────────────
    PIPELINE_EXPR,
    TERNARY_EXPR,
    BINARY_EXPR,
    UNARY_EXPR,
    POSTFIX_EXPR,
    PAREN_EXPR,
    PARTIAL_EXPR,
    RECORD_LITERAL,
    RECORD_FIELD,
    TRIPLE_TERM,
    ARG_LIST,
    ARG,
    NAMED_ARG,

    // ─── Phase 2: composite item nodes ────────────────────────────────
    IMPORT,
    IMPORT_PATH,
    SELECTIVE_IMPORT,
    ALIAS,
    DEFINITION,
    EXPORTED_DEFINITION,
    TYPE_ANNOTATION,
    TYPE_EXPR,
    TYPE_ATOM,
    SHAPE_EXPR,
    IN_CLAUSE,
    ANNOTATION_BLOCK,
    ANNOTATION_ITEM,

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
            // Phase 1 (IDs 0..=40)
            0 => Self::WHITESPACE,
            1 => Self::NEWLINE,
            2 => Self::COMMENT,
            3 => Self::IDENT,
            4 => Self::INTEGER,
            5 => Self::STRING,
            6 => Self::TEMPLATE,
            7 => Self::ABS_IRI,
            8 => Self::PREFIXED_NAME,
            9 => Self::FIELD_REF,
            10 => Self::DEFINE,
            11 => Self::ASSIGN,
            12 => Self::SHAPE_SEP,
            13 => Self::DOT,
            14 => Self::COMMA,
            15 => Self::LPAREN,
            16 => Self::RPAREN,
            17 => Self::LBRACE,
            18 => Self::RBRACE,
            19 => Self::LANGLE,
            20 => Self::RANGLE,
            21 => Self::KW_PREFIX,
            22 => Self::KW_FROM,
            23 => Self::INDENT,
            24 => Self::DEDENT,
            25 => Self::PROGRAM,
            26 => Self::PREFIX_DECL,
            27 => Self::SOURCE_DEF,
            28 => Self::MAPPING,
            29 => Self::MAPPING_HEADER,
            30 => Self::MAPPING_BODY,
            31 => Self::PROPERTY,
            32 => Self::PROPERTY_LHS,
            33 => Self::EXPR,
            34 => Self::CALL_EXPR,
            35 => Self::TEMPLATE_EXPR,
            36 => Self::IRI_EXPR,
            37 => Self::LITERAL_EXPR,
            38 => Self::FIELD_REF_EXPR,
            39 => Self::ERROR,
            40 => Self::EOF,
            // Phase 2: lexical tokens
            41 => Self::FLOAT,
            42 => Self::ENV_VAR,
            43 => Self::PARTIAL,
            44 => Self::AT_EXPORT,
            45 => Self::AT_ATTR,
            // Phase 2: punctuation / operators
            46 => Self::PIPE,
            47 => Self::ARROW,
            48 => Self::TYPE_ANNOT,
            49 => Self::TRIPLE_OPEN,
            50 => Self::TRIPLE_CLOSE,
            51 => Self::EQ,
            52 => Self::NEQ,
            53 => Self::LT,
            54 => Self::LE,
            55 => Self::GT,
            56 => Self::GE,
            57 => Self::PLUS,
            58 => Self::MINUS,
            59 => Self::STAR,
            60 => Self::SLASH,
            61 => Self::PERCENT,
            62 => Self::T_QUESTION,
            63 => Self::SHAPE_AND,
            // Phase 2: keywords
            64 => Self::KW_IN,
            65 => Self::KW_USE,
            66 => Self::KW_AS,
            67 => Self::KW_AND,
            68 => Self::KW_OR,
            69 => Self::KW_NOT,
            70 => Self::KW_IRI,
            // Phase 2: composite expression nodes
            71 => Self::PIPELINE_EXPR,
            72 => Self::TERNARY_EXPR,
            73 => Self::BINARY_EXPR,
            74 => Self::UNARY_EXPR,
            75 => Self::POSTFIX_EXPR,
            76 => Self::PAREN_EXPR,
            77 => Self::PARTIAL_EXPR,
            78 => Self::RECORD_LITERAL,
            79 => Self::RECORD_FIELD,
            80 => Self::TRIPLE_TERM,
            81 => Self::ARG_LIST,
            82 => Self::ARG,
            83 => Self::NAMED_ARG,
            // Phase 2: composite item nodes
            84 => Self::IMPORT,
            85 => Self::IMPORT_PATH,
            86 => Self::SELECTIVE_IMPORT,
            87 => Self::ALIAS,
            88 => Self::DEFINITION,
            89 => Self::EXPORTED_DEFINITION,
            90 => Self::TYPE_ANNOTATION,
            91 => Self::TYPE_EXPR,
            92 => Self::TYPE_ATOM,
            93 => Self::SHAPE_EXPR,
            94 => Self::IN_CLAUSE,
            95 => Self::ANNOTATION_BLOCK,
            96 => Self::ANNOTATION_ITEM,
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
