//! `SyntaxKind` — the kind tag for every node and token in the Fossil CST,
//! plus the `rowan::Language` impl that wires it into the green/red tree.
//!
//! Every variant here is a kind the lexer or the parser actually produces —
//! which is NOT the same as a terminal of `grammar.bnf`. Not one of them names a
//! retired spelling any more, and the grammar no longer has a terminal with no
//! kind here: `BOOL` was the last, and `true` / `false` arrived as `IDENT` until
//! it existed. The parser is what this enum describes.
//!
//! The `repr(u16)` values are an implementation detail of the rowan green
//! tree and are NOT a compatibility surface: nothing persists a raw value
//! across a build, so adding or removing a variant renumbers the rest, and
//! the only thing that has to move in lockstep is [`SyntaxKind::from_raw_value`]
//! (guarded by `syntax_kind_round_trip_for_all_variants`).
//!
//! The one cross-language pin is on the LEXER's `Token` discriminants, not on
//! these — see `packages/codemirror-fossil/src/tags.ts`.

// SCREAMING_SNAKE_CASE is the rust-analyzer / rowan-ecosystem convention for
// SyntaxKind variants, and it is the naming `grammar.bnf` uses for its
// terminals. Suppress the rustc style warning crate-wide for this enum only.
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
    /// `true` / `false` — `BOOL := 'true' | 'false'` (grammar.bnf, BOOL).
    ///
    /// ONE kind for both spellings: which one it is, is the token's text, and a
    /// pair of kinds would make every consumer match twice to learn one bit.
    BOOL,
    STRING,
    /// The opening `"` of an interpolated string.
    ///
    /// A string with no interpolation stays one `STRING` token: the carve
    /// happens only where there is something to carve, so every string that was
    /// one token before this existed is still one token.
    STRING_OPEN,
    /// A literal run between two interpolations, or between a delimiter and
    /// one. Carries its source text verbatim, `{{` included.
    STRING_TEXT,
    /// The opening of a hole — `{`. There is one spelling; `${` went with the
    /// backtick, and neither is a token.
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
    // There was a `PIPE` here — `|>`, retired as a second spelling of `a.f()`.
    // `|` matches no lexer rule now, so the bytes reach the parser as an ERROR
    // token and are refused by name.
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
    // There was a `PREFIX_DECL` here. `prefix ex: <http://example.org/>`
    // introduced the CURIE, and the CURIE is gone from every position it held —
    // a shape name, a property key, an expression and an interpolation hole are
    // bare names or full IRIs in strings now. A vocabulary declaration that
    // nothing spells is a statement about nothing.
    SOURCE_DEF,
    /// `{ A, B, ... } := io.rdf(uri, schema = shex)` — a destructuring source
    /// definition binding N members (one per declared shape) to a single source.
    MULTI_SOURCE_DEF,
    /// `type { A, B } := io.shex("s.shex")` — a type binding. The same
    /// destructuring as `MULTI_SOURCE_DEF`, in type position, and binding is
    /// POSITIONAL: the Nth name binds the Nth shape the document declares.
    ///
    /// The binder is `:=` — one binder, and what is being bound is read off the
    /// left-hand side (grammar.bnf, DEFINE). Every `=` in the
    /// grammar is an ASSIGNMENT: a body property, `@subject`, a named argument.
    /// Naming a shape document is MANDATORY: a bare
    /// property key takes its name from a predicate that a shape declares, so a
    /// program with no `type` binding cannot write a single property.
    ///
    /// A `TYPE_DEF` may carry [`SyntaxKind::RENAME_ATTR`] children BEFORE its
    /// `type` token — `RenameAttr*` is part of this production
    /// (grammar.bnf, `TypeDef`), not a top-level item of its own, because a
    /// `@rename` renames a predicate OF ONE BINDING and there is nowhere else
    /// for it to hang.
    TYPE_DEF,
    /// `policy := "people.jsonld"` — the ODRL document the release is verified
    /// against (grammar.bnf, `PolicyDef`). One per program, and the writer
    /// refuses to seal a corpus that misses the bound it declares.
    ///
    /// The right-hand side is a plain `STRING` and NOT an expression, so this
    /// node has exactly two token children that matter: the contextual `policy`
    /// and the reference. `io` is the type-provider registry — a row there runs
    /// at compile time and yields types — and the compiler never opens this
    /// document; the writer does, after the check. So there is no `io.policy`
    /// to call and no `EXPR` child to walk.
    ///
    /// It shares the `IDENT DEFINE` opening with [`SyntaxKind::SOURCE_DEF`] and
    /// the token AFTER `:=` decides (disambiguation rule 8; grammar.bnf,
    /// § DISAMBIGUATION RULES), which is what keeps `policy := io.csv("p.csv")`
    /// a source binding and `policy` an ordinary identifier everywhere.
    POLICY_DEF,
    /// `@rename(Person, "http://xmlns.com/foaf/0.1/name" as foaf_name)` —
    /// `RenameAttr := AT_ATTR LPAREN IDENT (COMMA Rename)+ RPAREN`
    /// (grammar.bnf, `RenameAttr`).
    ///
    /// The repair for two predicates whose last IRI segments coincide, in the
    /// shape of Prisma's `@map`: all constants, above the declaration, and in
    /// the PROGRAM rather than in the `.shex` because the vocabulary may not be
    /// yours. Its `IDENT` is one of the names the binding below it introduces.
    ///
    /// It shares its `AT_ATTR` token with `@subject` and POSITION tells them
    /// apart (disambiguation rule 6; grammar.bnf, § DISAMBIGUATION RULES):
    /// `@rename` above a `type` binding, `@subject` as the first line of a
    /// mapping body. Neither name is valid in the other's position and no third
    /// name is valid anywhere.
    RENAME_ATTR,
    /// One `"…" as name` inside a [`SyntaxKind::RENAME_ATTR`] —
    /// `Rename := STRING 'as' IDENT` (grammar.bnf, Rename).
    ///
    /// The first of the two places `as` survives, and one of the two that make
    /// it CONTEXTUAL rather than reserved: it is recognised between a `STRING`
    /// and a bare `IDENT`, where no expression could continue, so one token of
    /// lookahead settles it and `as` stays an ordinary identifier everywhere
    /// else (rule 7; grammar.bnf, § DISAMBIGUATION RULES).
    RENAME,
    MAPPING,
    MAPPING_HEADER,
    MAPPING_BODY,
    PROPERTY,
    PROPERTY_LHS,
    /// The mapping header's shape (grammar.bnf, `ShapeExpr`) — `ShapeExpr :=
    /// IDENT`. One `IDENT` token child and nothing else: one of the names a
    /// `type { … } := …` binding introduced. The `&` intersection went when the
    /// lowering was found to keep the first shape and drop the rest in silence,
    /// and the `IRI_EXPR` wrapper went with the CURIE.
    SHAPE_EXPR,

    // ─── Composite expression nodes ───────────────────────────────────
    EXPR,
    // There was a `TEMPLATE_EXPR` here — a backtick literal with no hole. The
    // backtick is not a token and `"…{expr}…"` is the one spelling, so there is
    // nothing left for a second node to be built from.
    /// A string with at least one hole — `STRING_OPEN (STRING_TEXT |
    /// INTERPOLATION)* STRING_CLOSE`. One spelling, one node.
    INTERP_STRING_EXPR,
    /// One hole — `INTERP_OPEN Expression RBRACE`. The expression is an
    /// ordinary expression, parsed by the ordinary expression parser: there is
    /// no format mini-language to keep in step with the checker.
    INTERPOLATION,
    // There was an `IRI_EXPR` here — the CURIE, the absolute IRI and the
    // backtick template, the three spellings of "an IRI written in the source".
    // A constant IRI is a STRING now, and the shape decides that it denotes
    // rather than reads.
    LITERAL_EXPR,
    // There was a `FIELD_REF_EXPR` here — `.name`, a column of an anonymous
    // current row. The row has a name now, so every reference is qualified and a
    // leading `.` starts nothing.
    // There was a `PIPELINE_EXPR` here — the node `a |> f()` built. The member
    // call is the spelling and `|>` is not a token, so the pipeline has no node.
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
    /// Same one-token lookahead as [`SyntaxKind::RENAME`]: an operand followed
    /// by a bare `IDENT`, which no expression can continue.
    ALIAS_ARG,

    // ─── Error / sentinel ─────────────────────────────────────────────
    ERROR,
    EOF,

    /// `null` — the absence of a value, `NULL := 'null'` (grammar.bnf).
    ///
    /// Declared here, after `EOF`, because the raw values below are positional
    /// and a kind inserted in the middle renumbers every one after it.
    NULL,

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
    /// the workspace `unsafe_code = "deny"` lint stays clean here. An
    /// `#[allow(unsafe_code)]` belongs only at a third-party-trait integration
    /// boundary; a value-to-enum decode is not one.
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
            6 => Self::BOOL,
            7 => Self::STRING,
            8 => Self::STRING_OPEN,
            9 => Self::STRING_TEXT,
            10 => Self::INTERP_OPEN,
            11 => Self::STRING_CLOSE,
            12 => Self::AT_ATTR,
            13 => Self::DEFINE,
            14 => Self::ASSIGN,
            15 => Self::SHAPE_SEP,
            16 => Self::DOT,
            17 => Self::COMMA,
            18 => Self::LPAREN,
            19 => Self::RPAREN,
            20 => Self::LBRACE,
            21 => Self::RBRACE,
            22 => Self::EQ,
            23 => Self::NEQ,
            24 => Self::LT,
            25 => Self::LE,
            26 => Self::GT,
            27 => Self::GE,
            28 => Self::PLUS,
            29 => Self::MINUS,
            30 => Self::STAR,
            31 => Self::SLASH,
            32 => Self::PERCENT,
            33 => Self::T_QUESTION,
            34 => Self::KW_FROM,
            35 => Self::KW_AND,
            36 => Self::KW_OR,
            37 => Self::KW_NOT,
            38 => Self::INDENT,
            39 => Self::DEDENT,
            40 => Self::PROGRAM,
            41 => Self::SOURCE_DEF,
            42 => Self::MULTI_SOURCE_DEF,
            43 => Self::TYPE_DEF,
            44 => Self::POLICY_DEF,
            45 => Self::RENAME_ATTR,
            46 => Self::RENAME,
            47 => Self::MAPPING,
            48 => Self::MAPPING_HEADER,
            49 => Self::MAPPING_BODY,
            50 => Self::PROPERTY,
            51 => Self::PROPERTY_LHS,
            52 => Self::SHAPE_EXPR,
            53 => Self::EXPR,
            54 => Self::INTERP_STRING_EXPR,
            55 => Self::INTERPOLATION,
            56 => Self::LITERAL_EXPR,
            57 => Self::TERNARY_EXPR,
            58 => Self::BINARY_EXPR,
            59 => Self::UNARY_EXPR,
            60 => Self::POSTFIX_EXPR,
            61 => Self::PAREN_EXPR,
            62 => Self::ARG_LIST,
            63 => Self::ARG,
            64 => Self::NAMED_ARG,
            65 => Self::ALIAS_ARG,
            66 => Self::ERROR,
            67 => Self::EOF,
            68 => Self::NULL,
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
