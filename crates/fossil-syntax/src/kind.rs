//! `SyntaxKind` — the kind tag for every node and token in the Fossil CST,
//! plus the `rowan::Language` impl that wires it into the green/red tree.
//!
//! Phase 1 carries the subset of grammar.bnf needed for `examples/hello.fossil`:
//! prefix decls, source defs, mappings (header + body of properties).
//! Phase 2+ adds operator expressions, ternaries, annotations, etc.

// SCREAMING_SNAKE_CASE is the rust-analyzer / rowan-ecosystem convention for
// SyntaxKind variants (matches the BNF terminal naming in `grammar.bnf`).
// Suppress the rustc style warning crate-wide for this enum only.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
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

    // Composite nodes
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
