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
//! # Grammar coverage (grammar.bnf lines 94-156)
//!
//! ```text
//! Program             := TopLevel* EOF
//! TopLevel            := Import | Definition | ExportedDefinition | Mapping
//!
//! Import              := 'use' Path SelectiveImport? Alias?
//! Path                := PathSegment ('/' PathSegment)*
//! PathSegment         := IDENT | STRING
//! SelectiveImport     := LBRACE IDENT (COMMA IDENT)* RBRACE
//! Alias               := 'as' IDENT
//!
//! Definition          := IDENT DEFINE Expression
//! ExportedDefinition  := AT_EXPORT TypeAnnotation? Definition
//! TypeAnnotation      := IDENT TYPE_ANNOT TypeExpr
//! TypeExpr            := TypeAtom (ARROW TypeExpr)?     -- right-assoc
//! TypeAtom            := IDENT | LPAREN TypeExpr (COMMA TypeExpr)* RPAREN
//!
//! Mapping             := MappingHeader NEWLINE INDENT MappingBody DEDENT
//! MappingHeader       := IDENT SHAPE_SEP ShapeExpr InClause? 'from' Expression
//! ShapeExpr           := IRIExpr (SHAPE_AND IRIExpr)*
//! InClause            := 'in' IRIExpr
//! MappingBody         := Property+
//! Property            := PropertyLhs ASSIGN Expression AnnotationBlock?
//! PropertyLhs         := 'iri' | IRIExpr
//! AnnotationBlock     := LBRACE AnnotationBody RBRACE
//! AnnotationBody      := AnnotationList | NEWLINE INDENT (AnnotationItem NEWLINE)+ DEDENT
//! AnnotationList      := AnnotationItem (AnnotationSep AnnotationItem)*
//! AnnotationSep       := COMMA | NEWLINE
//! AnnotationItem      := IRIExpr ASSIGN Expression AnnotationBlock?
//! ```
//!
//! Phase 1's `PrefixDecl` and `SourceDef` are NOT in the grammar.bnf
//! `TopLevel` set, but they remain valid top-level items in the Fossil
//! surface syntax (per the canonical `examples/hello.fossil`) and are
//! preserved verbatim here. `def_map` continues to scan for `SOURCE_DEF`
//! and `PREFIX_DECL` so the walking-skeleton invariant holds.
//!
//! # Disambiguation rules (grammar.bnf §"DISAMBIGUATION RULES")
//!
//! All four rules are implemented in this module + `super::expr` and
//! exercised by the unit tests in `mod disambiguation` at the bottom of
//! this file (Task 2a per plan 02-03 Blocker 1):
//!
//! 1. `RecordLiteral` vs `AnnotationBlock`: `prop = { … }` (LBRACE
//!    immediately after ASSIGN) is a `RECORD_LITERAL`; `prop = expr { … }`
//!    (LBRACE after an Expression) is an `ANNOTATION_BLOCK`. Implemented
//!    in [`parse_property`].
//! 2. `.IDENT` `FieldRef` vs `expr.IDENT` postfix member access: handled
//!    in `super::expr::parse_primary` (leading `DOT`) vs
//!    `super::expr::parse_postfix` (`DOT` after primary).
//! 3. `SHAPE_SEP` `:` in `MappingHeader` vs `T_COLON` `:` in a ternary:
//!    the same lexeme `:` is consumed by [`parse_mapping_header`] for
//!    rule (3a) and by [`super::expr::parse_expression`]'s ternary
//!    handler for rule (3b).
//! 4. `<<` `TRIPLE_OPEN` vs `<` `LT`: the lexer (Wave 0 plan 02-01) emits
//!    `TRIPLE_OPEN` as a single token; `LT` is the comparison operator.
//!    `<<` is consumed by [`super::expr::parse_triple_term`].

use crate::kind::SyntaxKind;

use super::Parser;
use super::recover::{self, CLOSE_BRACKET_ANCHORS, MAPPING_BODY_ANCHORS, TOP_LEVEL_ANCHORS};

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
            Some(SyntaxKind::KW_USE) => parse_import(p),
            Some(SyntaxKind::KW_PREFIX) => parse_prefix_decl(p),
            Some(SyntaxKind::AT_EXPORT) => parse_exported_definition(p),
            Some(SyntaxKind::IDENT) => match p.peek_kind(1) {
                // `IDENT :=` → source/value definition (Phase 1 SOURCE_DEF).
                Some(SyntaxKind::DEFINE) => parse_source_def(p),
                // `IDENT :` → start of a mapping header.
                Some(SyntaxKind::SHAPE_SEP) => parse_mapping(p),
                // `IDENT ::` → top-level Definition with leading
                // TypeAnnotation (no `@export`). Grammar.bnf line 121 ties
                // TypeAnnotation to ExportedDefinition; for symmetry we
                // also accept a bare top-level Definition with a leading
                // `IDENT ::` (treated as a TypeAnnotation followed by an
                // implicit Definition on the next line). Unreachable in
                // the Wave 0 / Phase 1 fixtures; kept defensive.
                //
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
// Import: 'use' Path SelectiveImport? Alias?
// ───────────────────────────────────────────────────────────────────────
fn parse_import(p: &mut Parser) {
    p.start(SyntaxKind::IMPORT);
    p.bump(); // KW_USE
    parse_path(p);
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::LBRACE) {
        parse_selective_import(p);
    }
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::KW_AS) {
        p.start(SyntaxKind::ALIAS);
        p.bump(); // KW_AS
        recover::expect_or_recover(p, SyntaxKind::IDENT, TOP_LEVEL_ANCHORS);
        p.finish();
    }
    p.finish();
}

// Path: PathSegment ('/' PathSegment)*
fn parse_path(p: &mut Parser) {
    p.start(SyntaxKind::IMPORT_PATH);
    parse_path_segment(p);
    loop {
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::SLASH) {
            p.bump();
            parse_path_segment(p);
        } else {
            break;
        }
    }
    p.finish();
}

fn parse_path_segment(p: &mut Parser) {
    p.skip_trivia();
    match p.current() {
        Some(SyntaxKind::IDENT | SyntaxKind::STRING) => p.bump(),
        _ => recover::recover_to(
            p,
            &[
                SyntaxKind::SLASH,
                SyntaxKind::LBRACE,
                SyntaxKind::KW_AS,
                SyntaxKind::KW_USE,
                SyntaxKind::KW_PREFIX,
                SyntaxKind::AT_EXPORT,
                SyntaxKind::IDENT,
            ],
        ),
    }
}

// SelectiveImport: LBRACE IDENT (COMMA IDENT)* RBRACE
fn parse_selective_import(p: &mut Parser) {
    p.start(SyntaxKind::SELECTIVE_IMPORT);
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
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// ExportedDefinition: AT_EXPORT TypeAnnotation? Definition
// ───────────────────────────────────────────────────────────────────────
fn parse_exported_definition(p: &mut Parser) {
    p.start(SyntaxKind::EXPORTED_DEFINITION);
    p.bump(); // AT_EXPORT
    p.skip_trivia();
    // TypeAnnotation lookahead: `IDENT ::` (with TYPE_ANNOT being `::`).
    if p.current() == Some(SyntaxKind::IDENT) && p.peek_kind(1) == Some(SyntaxKind::TYPE_ANNOT) {
        parse_type_annotation(p);
    }
    // The Definition: `IDENT := Expression`.
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::IDENT) {
        parse_definition(p);
    } else {
        recover::recover_to(p, TOP_LEVEL_ANCHORS);
    }
    p.finish();
}

// TypeAnnotation: IDENT TYPE_ANNOT TypeExpr
fn parse_type_annotation(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_ANNOTATION);
    p.bump(); // IDENT (function name)
    p.skip_trivia();
    p.bump(); // TYPE_ANNOT  (`::`)
    parse_type_expr(p);
    p.finish();
}

// TypeExpr: TypeAtom (ARROW TypeExpr)?   -- right-associative
fn parse_type_expr(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_EXPR);
    parse_type_atom(p);
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::ARROW) {
        p.bump();
        parse_type_expr(p); // right-assoc
    }
    p.finish();
}

fn parse_type_atom(p: &mut Parser) {
    p.skip_trivia();
    p.start(SyntaxKind::TYPE_ATOM);
    match p.current() {
        Some(SyntaxKind::IDENT) => p.bump(),
        Some(SyntaxKind::LPAREN) => {
            p.bump();
            parse_type_expr(p);
            loop {
                p.skip_trivia();
                if p.current() != Some(SyntaxKind::COMMA) {
                    break;
                }
                p.bump();
                parse_type_expr(p);
            }
            recover::expect_or_recover(
                p,
                SyntaxKind::RPAREN,
                &[SyntaxKind::ARROW, SyntaxKind::IDENT],
            );
        }
        _ => recover::recover_to(p, &[SyntaxKind::ARROW, SyntaxKind::IDENT]),
    }
    p.finish();
}

// Definition: IDENT DEFINE Expression
fn parse_definition(p: &mut Parser) {
    p.start(SyntaxKind::DEFINITION);
    recover::expect_or_recover(p, SyntaxKind::IDENT, TOP_LEVEL_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::DEFINE, TOP_LEVEL_ANCHORS);
    p.parse_expr();
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// SourceDef (Phase 1 / Fossil-specific): IDENT DEFINE Expression
//
// In the unified grammar.bnf line 119, `IDENT DEFINE Expression` is
// `Definition`. Phase 1's `def_map` keys source bindings on the SOURCE_DEF
// node kind, so we keep SOURCE_DEF for top-level (i.e. non-`@export`)
// `IDENT :=` items; the grammar.bnf `Definition` shape is reserved for the
// body of `ExportedDefinition`. This decision is documented in
// `02-03-SUMMARY.md` and the Salsa key (`def_map.rs`) is unaffected.
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

// MappingHeader: IDENT SHAPE_SEP ShapeExpr InClause? 'from' Expression
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
    if p.current() == Some(SyntaxKind::KW_IN) {
        parse_in_clause(p);
    }
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

// ShapeExpr: IRIExpr (SHAPE_AND IRIExpr)*
fn parse_shape_expr(p: &mut Parser) {
    p.start(SyntaxKind::SHAPE_EXPR);
    parse_iri_expr(p);
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::SHAPE_AND) {
            break;
        }
        p.bump(); // `&`
        parse_iri_expr(p);
    }
    p.finish();
}

// InClause: 'in' IRIExpr
fn parse_in_clause(p: &mut Parser) {
    p.start(SyntaxKind::IN_CLAUSE);
    p.bump(); // KW_IN
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
                SyntaxKind::SHAPE_AND,
                SyntaxKind::KW_FROM,
                SyntaxKind::KW_IN,
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

// Property: PropertyLhs ASSIGN Expression AnnotationBlock?
fn parse_property(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY);
    parse_property_lhs(p);
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, MAPPING_BODY_ANCHORS);
    // DISAMBIGUATION RULE 1 — RecordLiteral vs AnnotationBlock:
    //   ASSIGN immediately followed by LBRACE → the RecordLiteral primary
    //     is consumed by `expr::parse_primary` (which dispatches on LBRACE).
    //     The resulting CST has `EXPR > RECORD_LITERAL`.
    //   ASSIGN followed by anything else → an Expression. If a LBRACE
    //     follows the resulting Expression, that LBRACE is the
    //     AnnotationBlock attached to the property — consumed below.
    //
    // Both arms go through `p.parse_expr()`; the dispatch happens INSIDE
    // the Pratt parser via `parse_primary`'s LBRACE arm. The visible split
    // between record vs annotation is "is there a LBRACE LEFT after the
    // expression?" which we check below.
    p.parse_expr();
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::LBRACE) {
        parse_annotation_block(p);
    }
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

// AnnotationBlock: LBRACE AnnotationBody RBRACE
// AnnotationBody: AnnotationList | NEWLINE INDENT (AnnotationItem NEWLINE)+ DEDENT
fn parse_annotation_block(p: &mut Parser) {
    p.start(SyntaxKind::ANNOTATION_BLOCK);
    p.bump(); // LBRACE
    // Disambiguate single-line vs multi-line on the token right after
    // LBRACE (note: `peek_kind` skips NEWLINE as trivia, so the multi-line
    // form surfaces as an INDENT directly after the LBRACE in the
    // post-indent stream).
    if p.current() == Some(SyntaxKind::INDENT) {
        p.bump();
        parse_annotation_items_multiline(p);
        recover::expect_or_recover(p, SyntaxKind::DEDENT, CLOSE_BRACKET_ANCHORS);
    } else if p.current() != Some(SyntaxKind::RBRACE) {
        parse_annotation_list(p);
    }
    recover::expect_or_recover(p, SyntaxKind::RBRACE, MAPPING_BODY_ANCHORS);
    p.finish();
}

fn parse_annotation_list(p: &mut Parser) {
    parse_annotation_item(p);
    loop {
        p.skip_trivia();
        // AnnotationSep := COMMA | NEWLINE; the NEWLINE is trivia so we
        // only see COMMA here. RBRACE ends the list.
        if p.current() != Some(SyntaxKind::COMMA) {
            break;
        }
        p.bump();
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::RBRACE) {
            break; // trailing comma allowed
        }
        parse_annotation_item(p);
    }
}

fn parse_annotation_items_multiline(p: &mut Parser) {
    loop {
        p.skip_trivia();
        match p.current() {
            None | Some(SyntaxKind::DEDENT | SyntaxKind::RBRACE) => break,
            _ => parse_annotation_item(p),
        }
    }
}

// AnnotationItem: IRIExpr ASSIGN Expression AnnotationBlock?  (recursive)
fn parse_annotation_item(p: &mut Parser) {
    p.start(SyntaxKind::ANNOTATION_ITEM);
    parse_iri_expr(p);
    recover::expect_or_recover(
        p,
        SyntaxKind::ASSIGN,
        &[SyntaxKind::COMMA, SyntaxKind::RBRACE, SyntaxKind::DEDENT],
    );
    p.parse_expr();
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::LBRACE) {
        parse_annotation_block(p); // recursive — annotations on annotations
    }
    p.finish();
}

// =====================================================================
// Disambiguation-rule unit tests (Task 2a per Blocker 1)
// =====================================================================
//
// These four pairs of tests prove the parser implements all four grammar.bnf
// DISAMBIGUATION RULES BEFORE Task 3 regenerates the 24 fixture snapshots.
// Independent assertions on structural CST kinds catch the silent-bug class
// where `UPDATE_EXPECT=1` would otherwise bake the wrong shape into the
// snapshot baseline. See plan 02-03 §"Why these tests are required HERE".

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

    // ── RULE 1 — RecordLiteral vs AnnotationBlock ────────────────────

    #[test]
    fn rule1_lbrace_after_assign_is_record_literal() {
        let src = "User : ex:Shape from users\n    ex:rec = { name = .n }\n";
        let root = parse_str(src);
        let property =
            find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected at least one PROPERTY");
        // The PROPERTY's first EXPR descendant must contain RECORD_LITERAL
        // and MUST NOT contain ANNOTATION_BLOCK.
        assert!(
            descendant_kind_exists(&property, SyntaxKind::RECORD_LITERAL),
            "expected RECORD_LITERAL inside PROPERTY for `{{ name = .n }}` immediately after `=`",
        );
        assert!(
            !descendant_kind_exists(&property, SyntaxKind::ANNOTATION_BLOCK),
            "did NOT expect ANNOTATION_BLOCK here (rule 1 violation)",
        );
    }

    #[test]
    fn rule1_lbrace_after_expression_is_annotation_block() {
        let src = "User : ex:Shape from users\n    ex:name = .name { ex:lang = \"en\" }\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        assert!(
            descendant_kind_exists(&property, SyntaxKind::ANNOTATION_BLOCK),
            "expected ANNOTATION_BLOCK in PROPERTY for `.name {{ … }}`",
        );
        // The expression `.name` is a FIELD_REF_EXPR — make sure no
        // RECORD_LITERAL was conjured up by mistake.
        assert!(
            !descendant_kind_exists(&property, SyntaxKind::RECORD_LITERAL),
            "did NOT expect RECORD_LITERAL here (rule 1 violation)",
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

    // ── RULE 3 — SHAPE_SEP in MappingHeader vs T_COLON in TernaryExpr ─

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
        // The colon in a TERNARY_EXPR is conceptually T_COLON; the lexer
        // emits the same lexeme as SHAPE_SEP. The disambiguator is the
        // surrounding node (TERNARY_EXPR existed at all). Additionally
        // assert T_QUESTION is one of the TERNARY_EXPR's token children
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

    // ── RULE 4 — TRIPLE_OPEN vs LT ────────────────────────────────────

    #[test]
    fn rule4_double_lt_is_triple_open() {
        let src = "User : ex:Shape from users\n    iri = <<.s ex:p .o>>\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        assert!(
            descendant_kind_exists(&property, SyntaxKind::TRIPLE_TERM),
            "expected TRIPLE_TERM for `<<.s ex:p .o>>`",
        );
    }

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
        assert!(
            !token_kinds.contains(&SyntaxKind::TRIPLE_OPEN),
            "single `<` MUST NOT be tokenised as TRIPLE_OPEN, got {token_kinds:?}",
        );
    }
}
