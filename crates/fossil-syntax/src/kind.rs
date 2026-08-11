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
    /// The opening delimiter of an interpolated string — `"` or a backtick.
    ///
    /// A string with no interpolation stays one `STRING` (or `TEMPLATE`) token:
    /// the carve happens only where there is something to carve, so every
    /// string that was one token before this existed is still one token.
    STRING_OPEN,
    /// A literal run between two interpolations, or between a delimiter and
    /// one. Carries its source text verbatim, `{{` included.
    STRING_TEXT,
    /// The opening of a hole — `{`, or `${` in the backtick spelling that
    /// ADR-0057's seventh amendment retires.
    INTERP_OPEN,
    /// The closing delimiter. The hole's own `}` is an ordinary `RBRACE`.
    STRING_CLOSE,
    ABS_IRI,
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
    KW_PREFIX,
    KW_FROM,
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
    /// The mapping header's shape. One `IRI_EXPR` and only one: the `&`
    /// intersection went when the lowering was found to keep the first shape
    /// and drop the rest in silence.
    SHAPE_EXPR,

    // ─── Composite expression nodes ───────────────────────────────────
    EXPR,
    TEMPLATE_EXPR,
    /// A string with at least one hole — `STRING_OPEN (STRING_TEXT |
    /// INTERPOLATION)* STRING_CLOSE`. Both spellings produce this node.
    INTERP_STRING_EXPR,
    /// One hole — `INTERP_OPEN Expression RBRACE`. The expression is an
    /// ordinary expression, parsed by the ordinary expression parser: there is
    /// no format mini-language to keep in step with the checker.
    INTERPOLATION,
    IRI_EXPR,
    LITERAL_EXPR,
    FIELD_REF_EXPR,
    PIPELINE_EXPR,
    TERNARY_EXPR,
    BINARY_EXPR,
    UNARY_EXPR,
    POSTFIX_EXPR,
    PAREN_EXPR,
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
            8 => Self::STRING_OPEN,
            9 => Self::STRING_TEXT,
            10 => Self::INTERP_OPEN,
            11 => Self::STRING_CLOSE,
            12 => Self::ABS_IRI,
            13 => Self::AT_ATTR,
            14 => Self::DEFINE,
            15 => Self::ASSIGN,
            16 => Self::SHAPE_SEP,
            17 => Self::DOT,
            18 => Self::COMMA,
            19 => Self::LPAREN,
            20 => Self::RPAREN,
            21 => Self::LBRACE,
            22 => Self::RBRACE,
            23 => Self::PIPE,
            24 => Self::EQ,
            25 => Self::NEQ,
            26 => Self::LT,
            27 => Self::LE,
            28 => Self::GT,
            29 => Self::GE,
            30 => Self::PLUS,
            31 => Self::MINUS,
            32 => Self::STAR,
            33 => Self::SLASH,
            34 => Self::PERCENT,
            35 => Self::T_QUESTION,
            36 => Self::KW_PREFIX,
            37 => Self::KW_FROM,
            38 => Self::KW_AND,
            39 => Self::KW_OR,
            40 => Self::KW_NOT,
            41 => Self::KW_IRI,
            42 => Self::INDENT,
            43 => Self::DEDENT,
            44 => Self::PROGRAM,
            45 => Self::PREFIX_DECL,
            46 => Self::SOURCE_DEF,
            47 => Self::MULTI_SOURCE_DEF,
            48 => Self::TYPE_DEF,
            49 => Self::MAPPING,
            50 => Self::MAPPING_HEADER,
            51 => Self::MAPPING_BODY,
            52 => Self::PROPERTY,
            53 => Self::PROPERTY_LHS,
            54 => Self::SHAPE_EXPR,
            55 => Self::EXPR,
            56 => Self::TEMPLATE_EXPR,
            57 => Self::INTERP_STRING_EXPR,
            58 => Self::INTERPOLATION,
            59 => Self::IRI_EXPR,
            60 => Self::LITERAL_EXPR,
            61 => Self::FIELD_REF_EXPR,
            62 => Self::PIPELINE_EXPR,
            63 => Self::TERNARY_EXPR,
            64 => Self::BINARY_EXPR,
            65 => Self::UNARY_EXPR,
            66 => Self::POSTFIX_EXPR,
            67 => Self::PAREN_EXPR,
            68 => Self::ARG_LIST,
            69 => Self::ARG,
            70 => Self::NAMED_ARG,
            71 => Self::ERROR,
            72 => Self::EOF,
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
