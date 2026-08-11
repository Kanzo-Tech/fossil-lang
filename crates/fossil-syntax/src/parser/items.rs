// See `parser::expr` for the rationale on the redundant_pub_crate allow.
// `parser::items::parse_program` is called from `parser::Parser::parse_program`
// (the sibling), so the item needs `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

//! Item-level recursive-descent parser.
//!
//! Items are LL(1) on the leading token, so no backtracking is needed.
//! Per RESEARCH.md §Q1 we keep RD here and delegate every expression slot
//! to the Pratt sub-parser in [`super::expr::parse_expression`].
//!
//! # Grammar coverage (grammar.bnf §GRAMMAR)
//!
//! ```text
//! Program             := TopLevel* EOF
//! TopLevel            := PrefixDecl | SourceDef | MultiSourceDef | TypeDef | Mapping
//!
//! Mapping             := MappingHeader NEWLINE INDENT MappingBody DEDENT
//! MappingHeader       := IDENT SHAPE_SEP ShapeExpr 'from' Expression
//! ShapeExpr           := IRIExpr
//! MappingBody         := SubjectAttr? Property+
//! Property            := PropertyLhs ASSIGN Expression
//! PropertyLhs         := 'iri' | IRIExpr
//! ```
//!
//! `PrefixDecl`, `SourceDef` and `MultiSourceDef` are the vocabulary and
//! binding half of the surface; `def_map` scans for `SOURCE_DEF` and
//! `PREFIX_DECL`, which is what the walking-skeleton invariant rests on.
//!
//! There is no `parse_import`. `use foo/bar { a, b } as c` built four node
//! kinds and no stage below the parser read one of them; ADR-0057's second
//! amendment keeps `use` dead until there is a module system for a name to
//! come from.
//!
//! # Disambiguation rules (grammar.bnf §"DISAMBIGUATION RULES")
//!
//! The surviving rules are implemented in this module + `super::expr` and
//! exercised by the unit tests in `mod disambiguation` at the bottom of
//! this file (Task 2a per plan 02-03 Blocker 1):
//!
//! (Rule 1 was `RecordLiteral` vs `AnnotationBlock` — whether the `{` came
//! straight after `=` or after an expression. Both forms went, and telling
//! two absent things apart is not a rule. A `{` in a mapping body is now
//! one thing: an error.)
//! 2. `.IDENT` `FieldRef` vs `expr.IDENT` postfix member access: handled
//!    in `super::expr::parse_primary` (leading `DOT`) vs
//!    `super::expr::parse_postfix` (`DOT` after primary).
//! 3. `:` in a `MappingHeader` vs `:` in a ternary: one lexeme, one token
//!    (`SHAPE_SEP`), consumed by [`parse_mapping_header`] in the first case
//!    and by [`super::expr::parse_expression`]'s ternary handler in the
//!    second. The node is what tells them apart, not the token.
//! (Rule 4 was `<<` vs `<`, and it is gone with the triple term.)

use crate::kind::SyntaxKind;

use super::Parser;
use super::recover::{self, MAPPING_BODY_ANCHORS, TOP_LEVEL_ANCHORS};

/// Top-level entry point for the item parser. Wraps a `PROGRAM` node around
/// a `*`-loop over `TopLevel` productions. Called by `Parser::parse_program`
/// which itself is the body of the Salsa-tracked `parse` query.
pub(crate) fn parse_program(p: &mut Parser) {
    p.start(SyntaxKind::PROGRAM);
    loop {
        p.skip_trivia();
        // Drop stray virtual indentation tokens at the top level — DEDENTs
        // after the last mapping reach here, and a stray INDENT can appear
        // when a continuation line is mis-indented inside an otherwise-
        // recovered region. Both are benign at the program level.
        if matches!(p.current(), Some(SyntaxKind::INDENT | SyntaxKind::DEDENT)) {
            p.bump();
            continue;
        }
        match p.current() {
            None => break,
            Some(SyntaxKind::KW_PREFIX) => parse_prefix_decl(p),
            // `{ A, B, ... } := io.rdf(...)` — a destructuring source def. A
            // top-level `{` is the only `{` there is now: the selective-import
            // brace went with `use`, and the record and annotation braces went
            // with the forms that opened them.
            Some(SyntaxKind::LBRACE) => parse_multi_source_def(p),
            Some(SyntaxKind::IDENT) => match p.peek_kind(1) {
                // `IDENT :=` → source binding (SOURCE_DEF).
                Some(SyntaxKind::DEFINE) => parse_source_def(p),
                // `IDENT :` → start of a mapping header.
                Some(SyntaxKind::SHAPE_SEP) => parse_mapping(p),
                // `type { A, B } = …` → a type binding (TYPE_DEF). `type` is a
                // CONTEXTUAL keyword, not a reserved word: the slot `IDENT {`
                // was free at top level, and any other identifier followed by
                // `{` still falls through to the error arm below. So a column
                // or binding called `type` keeps working, which matters where
                // `rdf:type` is the commonest predicate there is.
                Some(SyntaxKind::LBRACE) if p.current_text() == Some("type") => {
                    parse_type_def(p);
                }
                // Always make progress on a token we don't know what to do
                // with at the program level — `bump_as_error` emits a single
                // ERROR token and advances, making the outer loop monotone
                // in `p.pos`. We CANNOT call `recover_to(p, TOP_LEVEL_ANCHORS)`
                // here because `IDENT` is itself in `TOP_LEVEL_ANCHORS` and
                // `recover_to` would no-op while the outer `loop` re-enters
                // this arm forever (see `.planning/phases/06-cli-complete-lsp/
                // deferred-items.md` — parser-hang on bare `#`, the bytes
                // logos drops silently which leave IDENT as the next token).
                _ => p.bump_as_error(),
            },
            // Always make progress on a token we don't know what to do with
            // at the program level — same invariant as the IDENT-arm above.
            // The dropped `#` becomes lexer-silence; the next non-trivia
            // token reaches this outer `_` and MUST be consumed under an
            // ERROR node so the outer `loop` advances.
            _ => p.bump_as_error(),
        }
    }
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// SourceDef: IDENT DEFINE Expression
//
// The one binding form. `users := io.csv("u.csv")` reads a file and
// `adultos := users |> where(...)` derives a relation; both are a name, `:=`
// and an expression, and `def_map` keys both on SOURCE_DEF.
// ───────────────────────────────────────────────────────────────────────
fn parse_source_def(p: &mut Parser) {
    p.start(SyntaxKind::SOURCE_DEF);
    p.bump(); // IDENT  (already verified by parse_program lookahead)
    p.skip_trivia();
    p.bump(); // DEFINE  (`:=`)
    p.parse_expr();
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// MultiSourceDef (destructuring source): `{ A, B, ... } := io.rdf(...)`
//   MultiSourceDef := LBRACE IDENT (COMMA IDENT)* RBRACE DEFINE Expression
//
// Each IDENT in the brace list names a member bound to the single source on
// the right of `:=` (the local-name of a shape declared in the source's
// schema). The brace/comma loop is shared with `parse_type_def`; the `:= expr`
// tail mirrors `parse_source_def`.
// ───────────────────────────────────────────────────────────────────────
fn parse_multi_source_def(p: &mut Parser) {
    p.start(SyntaxKind::MULTI_SOURCE_DEF);
    p.bump(); // LBRACE
    recover::expect_or_recover(
        p,
        SyntaxKind::IDENT,
        &[SyntaxKind::COMMA, SyntaxKind::RBRACE],
    );
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::COMMA) {
            break;
        }
        p.bump(); // COMMA
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::RBRACE) {
            break; // trailing comma allowed
        }
        recover::expect_or_recover(
            p,
            SyntaxKind::IDENT,
            &[SyntaxKind::COMMA, SyntaxKind::RBRACE],
        );
    }
    recover::expect_or_recover(p, SyntaxKind::RBRACE, TOP_LEVEL_ANCHORS);
    p.skip_trivia();
    recover::expect_or_recover(p, SyntaxKind::DEFINE, TOP_LEVEL_ANCHORS);
    p.parse_expr();
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// TypeDef (type binding): `type { Person, City } = io.shex("s.shex")`
//   TypeDef := 'type' LBRACE IDENT (COMMA IDENT)* RBRACE ASSIGN Expression
//
// The same destructuring as `MultiSourceDef`, in type position — one catalogue
// (`io.*`), two binders, `:=` for values and `type … =` for types (ADR-0057,
// seventh amendment). The brace list is byte-for-byte the loop above; the two
// differences are the leading contextual `type` and `=` instead of `:=`.
//
// `=` and not `:=` on purpose: `:=` binds a value and this binds a type, so
// giving them one spelling would be the one-idea-one-spelling rule read
// backwards — two ideas wearing the same glyph.
// ───────────────────────────────────────────────────────────────────────
fn parse_type_def(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_DEF);
    p.bump(); // IDENT `type` — contextual, checked by the caller
    p.skip_trivia();
    p.bump(); // LBRACE — the caller's lookahead already saw it
    recover::expect_or_recover(
        p,
        SyntaxKind::IDENT,
        &[SyntaxKind::COMMA, SyntaxKind::RBRACE],
    );
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::COMMA) {
            break;
        }
        p.bump(); // COMMA
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::RBRACE) {
            break; // trailing comma allowed
        }
        recover::expect_or_recover(
            p,
            SyntaxKind::IDENT,
            &[SyntaxKind::COMMA, SyntaxKind::RBRACE],
        );
    }
    recover::expect_or_recover(p, SyntaxKind::RBRACE, TOP_LEVEL_ANCHORS);
    p.skip_trivia();
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, TOP_LEVEL_ANCHORS);
    p.parse_expr();
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// PrefixDecl (Phase 1 / Fossil-specific, not in grammar.bnf §TopLevel
// but preserved as a top-level item in the surface syntax):
//   'prefix' IDENT SHAPE_SEP ABS_IRI
// ───────────────────────────────────────────────────────────────────────
fn parse_prefix_decl(p: &mut Parser) {
    p.start(SyntaxKind::PREFIX_DECL);
    p.bump(); // KW_PREFIX
    recover::expect_or_recover(p, SyntaxKind::IDENT, TOP_LEVEL_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::SHAPE_SEP, TOP_LEVEL_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::ABS_IRI, TOP_LEVEL_ANCHORS);
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// Mapping: MappingHeader NEWLINE INDENT MappingBody DEDENT
// ───────────────────────────────────────────────────────────────────────
fn parse_mapping(p: &mut Parser) {
    p.start(SyntaxKind::MAPPING);
    parse_mapping_header(p);
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::INDENT) {
        p.bump();
        parse_mapping_body(p);
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::DEDENT) {
            p.bump();
        } else {
            // DEDENT missing — typically because the user de-indented to
            // column 0, in which case the indent pass already emitted a
            // DEDENT we just consumed. Otherwise recovery falls through.
            recover::recover_to(p, TOP_LEVEL_ANCHORS);
        }
    } else {
        // Body missing — emit recovery and continue past. Grammar.bnf says
        // MappingBody := Property+ (1-or-more); a missing body is a parse
        // error.
        recover::recover_to(p, TOP_LEVEL_ANCHORS);
    }
    p.finish();
}

// MappingHeader: IDENT SHAPE_SEP ShapeExpr 'from' Expression
fn parse_mapping_header(p: &mut Parser) {
    p.start(SyntaxKind::MAPPING_HEADER);
    recover::expect_or_recover(
        p,
        SyntaxKind::IDENT,
        &[SyntaxKind::SHAPE_SEP, SyntaxKind::KW_FROM],
    );
    recover::expect_or_recover(
        p,
        SyntaxKind::SHAPE_SEP,
        &[SyntaxKind::KW_FROM, SyntaxKind::INDENT],
    );
    parse_shape_expr(p);
    p.skip_trivia();
    // `from` is required by the grammar but absent in some recovery
    // fixtures (e.g. fixture 16). Emit an ExpectedToken diagnostic and
    // recover toward INDENT or the next top-level anchor.
    recover::expect_or_recover(p, SyntaxKind::KW_FROM, &[SyntaxKind::INDENT]);
    // The `from` source can be a full Expression (grammar.bnf line 130 was
    // changed from IDENT to Expression in plan 02-03). For Phase 1's
    // hello.fossil shape (`from users`) this still parses cleanly because
    // a bare IDENT is a valid primary expression.
    p.skip_trivia();
    if p.current() != Some(SyntaxKind::INDENT) && p.current().is_some() {
        p.parse_expr();
    }
    p.finish();
}

// ShapeExpr: IRIExpr
//
// One shape. The `(SHAPE_AND IRIExpr)*` tail parsed an intersection the HIR
// then reduced to its first element, silently — so `User : ex:Person &
// ex:Employee` checked against `ex:Person` alone and nobody was told. The node
// stays because the header needs a place for its one shape; the `&` does not.
fn parse_shape_expr(p: &mut Parser) {
    p.start(SyntaxKind::SHAPE_EXPR);
    parse_iri_expr(p);
    p.finish();
}

// IRIExpr: ABS_IRI | TEMPLATE | PrefixedName
// PrefixedName: IDENT SHAPE_SEP IDENT  (lexer-contiguous; no whitespace)
#[allow(clippy::match_same_arms)] // ABS_IRI/TEMPLATE and bare-IDENT share `bump()` but represent semantically distinct IRIExpr forms; merging the arms would obscure the disambiguation rules
fn parse_iri_expr(p: &mut Parser) {
    p.skip_trivia();
    p.start(SyntaxKind::IRI_EXPR);
    match p.current() {
        Some(SyntaxKind::ABS_IRI | SyntaxKind::TEMPLATE) => p.bump(),
        Some(SyntaxKind::IDENT)
            if p.peek_contiguous(3)
                && p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
                && p.peek_kind(2) == Some(SyntaxKind::IDENT) =>
        {
            // Lexer-contiguous PrefixedName: `ex:Foo`. Three bumps for
            // the IDENT, SHAPE_SEP, and IDENT.
            p.bump();
            p.bump();
            p.bump();
        }
        // Tolerate a bare IDENT (no prefix) — `Foo` alone — as a degenerate
        // IRIExpr so a missing colon doesn't cascade. The grammar permits
        // this via the unprefixed shape name shorthand the standard uses.
        Some(SyntaxKind::IDENT) => p.bump(),
        _ => recover::recover_to(
            p,
            &[
                SyntaxKind::KW_FROM,
                SyntaxKind::ASSIGN,
                SyntaxKind::COMMA,
                SyntaxKind::RBRACE,
                SyntaxKind::INDENT,
                SyntaxKind::DEDENT,
            ],
        ),
    }
    p.finish();
}

// MappingBody: Property+
fn parse_mapping_body(p: &mut Parser) {
    p.start(SyntaxKind::MAPPING_BODY);
    loop {
        p.skip_trivia();
        match p.current() {
            None | Some(SyntaxKind::DEDENT) => break,
            Some(SyntaxKind::AT_ATTR) => parse_subject_attr(p),
            Some(SyntaxKind::IDENT | SyntaxKind::KW_IRI | SyntaxKind::ABS_IRI) => {
                parse_property(p);
            }
            _ => {
                // Malformed LHS — recover to either a fresh property start
                // (next IDENT/KW_IRI) or the body's closing DEDENT.
                recover::recover_to(p, MAPPING_BODY_ANCHORS);
                // Guard against the no-progress case: if recover_to left us
                // on the same token (already an anchor) but it is NOT a
                // valid property starter, force a single-token bump under an
                // ERROR node so the loop terminates.
                if !matches!(
                    p.current(),
                    None | Some(
                        SyntaxKind::DEDENT
                            | SyntaxKind::IDENT
                            | SyntaxKind::KW_IRI
                            | SyntaxKind::ABS_IRI
                    )
                ) {
                    p.bump_as_error();
                }
            }
        }
    }
    p.finish();
}

/// `@subject(iri = Expression)` — the mapping's subject, as the first line of
/// the body (ADR-0057, seventh amendment §4).
///
/// It is the FIRST production to accept the `@` sigil; until now `grammar.bnf`
/// said outright that `@name` lexes as `AT_ATTR` and nothing accepts it. It is
/// syntax with a sigil, NOT an attribute: the amendment's Rule B says a thing
/// required in every declaration is not an attribute, and attributes may carry
/// only constants while this dereferences the row.
///
/// It goes inside the body rather than above the header because the row binder
/// is introduced BY the header — above it, `u` does not exist yet.
///
/// It produces the same `PROPERTY` node as `iri = …`, so nothing below the
/// parser learns there are two spellings. `iri` stays a keyword until the
/// fixtures stop using it.
fn parse_subject_attr(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY);
    p.start(SyntaxKind::PROPERTY_LHS);
    p.bump(); // AT_ATTR
    p.finish();
    recover::expect_or_recover(p, SyntaxKind::LPAREN, MAPPING_BODY_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::KW_IRI, MAPPING_BODY_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, MAPPING_BODY_ANCHORS);
    p.parse_expr();
    recover::expect_or_recover(p, SyntaxKind::RPAREN, MAPPING_BODY_ANCHORS);
    p.finish();
}

// Property: PropertyLhs ASSIGN Expression
//
// There is no `AnnotationBlock?` tail and no record-literal primary, so a `{`
// anywhere in a property is an error rather than a fork. The old disambiguation
// rule 1 asked which of the two a `{` opened; with neither of them left the
// question has one answer, and `parse_mapping_body`'s recovery arm gives it.
fn parse_property(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY);
    parse_property_lhs(p);
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, MAPPING_BODY_ANCHORS);
    p.parse_expr();
    p.finish();
}

// PropertyLhs: 'iri' | IRIExpr
fn parse_property_lhs(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY_LHS);
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::KW_IRI) {
        p.bump();
    } else {
        parse_iri_expr(p);
    }
    p.finish();
}

// =====================================================================
// Disambiguation-rule unit tests (Task 2a per Blocker 1)
// =====================================================================
//
// These tests prove the parser implements the grammar.bnf DISAMBIGUATION
// RULES BEFORE Task 3 regenerates the fixture snapshots. Independent
// assertions on structural CST kinds catch the silent-bug class where
// `UPDATE_EXPECT=1` would otherwise bake the wrong shape into the snapshot
// baseline. See plan 02-03 §"Why these tests are required HERE".

#[cfg(test)]
mod disambiguation {
    use crate::SyntaxKind;
    use crate::indent::lex_with_indents;
    use crate::kind::SyntaxNode;
    use rowan::GreenNode;

    use super::super::Parser;

    fn parse_str(src: &str) -> SyntaxNode {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        super::parse_program(&mut p);
        let green: GreenNode = p.builder.finish();
        SyntaxNode::new_root(green)
    }

    fn find_first_kind(root: &SyntaxNode, want: SyntaxKind) -> Option<SyntaxNode> {
        if root.kind() == want {
            return Some(root.clone());
        }
        for child in root.children() {
            if let Some(n) = find_first_kind(&child, want) {
                return Some(n);
            }
        }
        None
    }

    fn descendant_kind_exists(root: &SyntaxNode, want: SyntaxKind) -> bool {
        find_first_kind(root, want).is_some()
    }

    // ── `{` claims nothing in a property, in either position ──────────
    //
    // There used to be a rule 1 here: `prop = { … }` was a RECORD_LITERAL and
    // `prop = expr { … }` was an ANNOTATION_BLOCK, and a pair of tests pinned
    // that each was not the other. Both forms went — the record literal is a
    // blank node the corpus has no row for, the annotation block a statement
    // about a statement with no term to name one. With neither left, a rule
    // that told them apart asserts nothing.
    //
    // What survives is the one answer that replaced the fork: in a property, a
    // `{` opens nothing at all, in EITHER position. That is worth pinning,
    // because a language where a brace has exactly zero readings is what makes
    // deleting the rule safe.

    #[test]
    fn brace_after_assign_is_an_error() {
        let src = "User : ex:Shape from users\n    ex:rec = { name = .n }\n";
        let root = parse_str(src);
        assert!(
            descendant_kind_exists(&root, SyntaxKind::ERROR),
            "`= {{ … }}` must be an error: nothing opens a brace in value position",
        );
    }

    #[test]
    fn brace_after_an_expression_is_an_error() {
        let src = "User : ex:Shape from users\n    ex:name = .name { ex:lang = \"en\" }\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        // The property itself still parses — `.name` is its whole value.
        assert!(
            descendant_kind_exists(&property, SyntaxKind::FIELD_REF_EXPR),
            "expected the property's value `.name` to parse as a FIELD_REF_EXPR",
        );
        assert!(
            descendant_kind_exists(&root, SyntaxKind::ERROR),
            "`.name {{ … }}` must be an error: the annotation block is gone",
        );
    }

    // ── RULE 2 — FieldRef vs PostfixOp ────────────────────────────────

    #[test]
    fn rule2_leading_dot_ident_is_field_ref() {
        let src = "User : ex:Shape from users\n    ex:x = .name\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        assert!(
            descendant_kind_exists(&property, SyntaxKind::FIELD_REF_EXPR),
            "expected FIELD_REF_EXPR for leading `.name`",
        );
    }

    #[test]
    fn rule2_dot_after_ident_is_postfix_method_access() {
        let src = "User : ex:Shape from users\n    ex:x = obj.method()\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        assert!(
            descendant_kind_exists(&property, SyntaxKind::POSTFIX_EXPR),
            "expected POSTFIX_EXPR for `obj.method()`",
        );
    }

    // ── RULE 3 — `:` in a MappingHeader vs `:` in a ternary ───────────

    #[test]
    fn rule3_colon_in_mapping_header_is_shape_sep() {
        let src = "User : ex:Shape from users\n    iri = .id\n";
        let root = parse_str(src);
        let header =
            find_first_kind(&root, SyntaxKind::MAPPING_HEADER).expect("expected a MAPPING_HEADER");
        let token_kinds: Vec<SyntaxKind> = header
            .children_with_tokens()
            .filter_map(crate::SyntaxElement::into_token)
            .map(|t| t.kind())
            .collect();
        assert!(
            token_kinds.contains(&SyntaxKind::SHAPE_SEP),
            "expected SHAPE_SEP token in MAPPING_HEADER",
        );
    }

    #[test]
    fn rule3_colon_in_ternary_is_t_colon() {
        let src = "User : ex:Shape from users\n    ex:x = cond ? a : b\n";
        let root = parse_str(src);
        let ternary =
            find_first_kind(&root, SyntaxKind::TERNARY_EXPR).expect("expected TERNARY_EXPR");
        // The colon in a TERNARY_EXPR is a SHAPE_SEP — the same token the
        // header takes. The grammar used to declare a `T_COLON` terminal for
        // this position and nothing ever produced one, so the disambiguator
        // was always the surrounding node (TERNARY_EXPR existed at all), never
        // the token. Additionally assert T_QUESTION is one of its token children
        // — without `?` there is no ternary, by construction.
        let token_kinds: Vec<SyntaxKind> = ternary
            .children_with_tokens()
            .filter_map(crate::SyntaxElement::into_token)
            .map(|t| t.kind())
            .collect();
        assert!(
            token_kinds.contains(&SyntaxKind::T_QUESTION),
            "expected T_QUESTION token in TERNARY_EXPR, got {token_kinds:?}",
        );
    }

    // ── `<` is the only thing `<` can be ──────────────────────────────
    //
    // There used to be a rule 4 here — `<<` was `TRIPLE_OPEN`, and a test
    // pinned that a single `<` was not mistaken for it. The triple term went
    // with the RDF-specific surface, so `<<` claims nothing and the
    // disambiguation it needed is gone. What survives is the half worth
    // keeping: `<` is a comparison operator.

    #[test]
    fn rule4_single_lt_is_comparison_operator() {
        let src = "User : ex:Shape from users\n    ex:x = a < b\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        let binary =
            find_first_kind(&property, SyntaxKind::BINARY_EXPR).expect("expected BINARY_EXPR");
        let token_kinds: Vec<SyntaxKind> = binary
            .children_with_tokens()
            .filter_map(crate::SyntaxElement::into_token)
            .map(|t| t.kind())
            .collect();
        assert!(
            token_kinds.contains(&SyntaxKind::LT),
            "expected LT token in BINARY_EXPR, got {token_kinds:?}",
        );
    }
}
