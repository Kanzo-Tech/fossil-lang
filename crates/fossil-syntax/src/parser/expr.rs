// The parent `parser` module declares this submodule `pub(crate) mod expr;`
// (private to the crate). Items inside that need to be reachable from sibling
// submodules (e.g. `parser::items` calls `parse_expression`) must be
// `pub(crate)` to satisfy the `unreachable_pub` lint; that combo trips the
// inverse `redundant_pub_crate` lint, which we silence here. The visibility
// IS correct — both lints can't be satisfied simultaneously for a sibling-
// callable item in a `pub(crate)` submodule.
#![allow(clippy::redundant_pub_crate)]

//! Pratt expression sub-parser implementing grammar.bnf §"OPERATOR
//! PRECEDENCE TABLE" verbatim.
//!
//! Entry point: [`parse_expression`].
//!
//! Reference: Crafting Interpreters Ch. 17 (Nystrom) + rust-analyzer's
//! `crates/parser/src/grammar/expressions.rs`. Per RESEARCH.md §Q1 the
//! Pratt shape is materially cleaner than nested-precedence-functions for
//! 9 levels.
//!
//! Binding-power table (lbp = left binding power; rbp = right binding
//! power; tighter operators → higher numbers):
//!
//! | BNF | Op                     | Assoc       | lbp | rbp |
//! |-----|------------------------|-------------|-----|-----|
//! | L1  | `\|>`                  | left        |   1 |   2 |
//! | L2  | `? :`                  | right       |   4 |   3 |
//! | L3  | `or`                   | left        |   5 |   6 |
//! | L4  | `and`                  | left        |   7 |   8 |
//! | L5  | `== != < <= > >=`      | non-assoc   |   9 |  10 |
//! | L6  | `+ -`                  | left        |  11 |  12 |
//! | L7  | `* / %`                | left        |  13 |  14 |
//! | L8  | unary `-`, `not`       | right       |  —  |  15 |
//! | L9  | `.` member, `()` call  | left        |  17 |  —  |
//!
//! Left-associative operators give `rbp = lbp + 1`; right-associative
//! operators give `rbp = lbp - 1`. Non-associative operators check that
//! the LHS was not already produced at the same level and bail with an
//! ERROR otherwise.

use crate::kind::SyntaxKind;

use super::Parser;

/// Left-binding-power below which the Pratt loop will bail out (the caller's
/// outer level wants control back).
type Bp = u8;

/// Associativity tag returned alongside binding-power. Left- and
/// right-associativity are encoded structurally in the `(lbp, rbp)` pair
/// (`rbp = lbp+1` for left, `rbp = lbp-1` for right) so the enum only
/// needs to distinguish "non-assoc" from "associative" for the chain-
/// rejection latch.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Assoc {
    /// Left- or right-associative — distinguished by the `rbp` relation.
    Assoc,
    /// Non-associative (the L5 comparison operators in grammar.bnf).
    Non,
}

/// Pratt parse an expression, stopping at any infix operator whose left
/// binding power is `< min_bp`. Top-level callers pass `0`.
pub(crate) fn parse_expression(p: &mut Parser, min_bp: Bp) {
    p.skip_trivia();
    let cp = p.checkpoint();
    parse_unary_or_primary(p);
    // Set to `Some(lbp)` after consuming a non-associative operator; if the
    // next infix is the same level (or another non-assoc at the same lbp)
    // we MUST refuse it — this is how `a < b < c` parses as
    // `BINARY_EXPR(a, b)` followed by stray `< c` siblings instead of
    // chaining.
    let mut just_consumed_non_assoc: Option<Bp> = None;

    loop {
        p.skip_trivia();
        // Ternary: handled separately because it's a 3-operand infix that
        // Pratt's two-arg lookup table cannot represent directly.
        if matches!(p.current(), Some(SyntaxKind::T_QUESTION)) && TERNARY_LBP >= min_bp {
            p.start_at(cp, SyntaxKind::TERNARY_EXPR);
            p.bump(); // `?`
            parse_expression(p, TERNARY_RBP + 1); // then-branch — cannot itself be a ternary at the same level
            p.skip_trivia();
            // Disambiguation rule #3 (grammar.bnf line 251): `:` in this
            // context is T_COLON, paired with the just-consumed `?`. The
            // lexer emits it as `SHAPE_SEP`; we consume it as such here.
            if p.current() == Some(SyntaxKind::SHAPE_SEP) {
                p.bump();
            } else {
                p.start(SyntaxKind::ERROR);
                p.finish();
            }
            parse_expression(p, TERNARY_RBP); // else-branch — right-assoc: another ternary OK
            p.finish();
            just_consumed_non_assoc = None;
            continue;
        }

        let Some((lbp, rbp, assoc, wrapper)) = peek_infix(p) else {
            break;
        };
        if lbp < min_bp {
            break;
        }
        // Non-assoc anti-chain: refuse another non-assoc op at the same lbp
        // that we just consumed (`a < b < c` parses as `(a < b) < c` only
        // if we allow it; the grammar says we don't).
        if assoc == Assoc::Non && just_consumed_non_assoc == Some(lbp) {
            break;
        }
        p.start_at(cp, wrapper);
        p.bump(); // operator
        parse_expression(p, rbp);
        p.finish();
        just_consumed_non_assoc = if assoc == Assoc::Non { Some(lbp) } else { None };
    }
}

const TERNARY_LBP: Bp = 4;
const TERNARY_RBP: Bp = 3;

/// Returns `(left_bp, right_bp, associativity, wrapper_kind)` for the current
/// non-trivia token. `None` if it is not an infix operator we recognise.
fn peek_infix(p: &Parser) -> Option<(Bp, Bp, Assoc, SyntaxKind)> {
    let k = p.current()?;
    Some(match k {
        // L1
        SyntaxKind::PIPE => (1, 2, Assoc::Assoc, SyntaxKind::PIPELINE_EXPR),
        // L3
        SyntaxKind::KW_OR => (5, 6, Assoc::Assoc, SyntaxKind::BINARY_EXPR),
        // L4
        SyntaxKind::KW_AND => (7, 8, Assoc::Assoc, SyntaxKind::BINARY_EXPR),
        // L5 (non-associative)
        SyntaxKind::EQ
        | SyntaxKind::NEQ
        | SyntaxKind::LT
        | SyntaxKind::LE
        | SyntaxKind::GT
        | SyntaxKind::GE => (9, 10, Assoc::Non, SyntaxKind::BINARY_EXPR),
        // L6
        SyntaxKind::PLUS | SyntaxKind::MINUS => (11, 12, Assoc::Assoc, SyntaxKind::BINARY_EXPR),
        // L7
        SyntaxKind::STAR | SyntaxKind::SLASH | SyntaxKind::PERCENT => {
            (13, 14, Assoc::Assoc, SyntaxKind::BINARY_EXPR)
        }
        _ => return None,
    })
}

/// L8 unary or fall through to L9 primary + postfix.
fn parse_unary_or_primary(p: &mut Parser) {
    p.skip_trivia();
    match p.current() {
        Some(SyntaxKind::MINUS | SyntaxKind::KW_NOT) => {
            p.start(SyntaxKind::UNARY_EXPR);
            p.bump(); // unary op
            parse_unary_or_primary(p); // right-associative recurse
            p.finish();
        }
        _ => parse_postfix(p),
    }
}

/// L9 postfix: primary followed by `.IDENT` member access or `(args)` call.
/// Left-associative by virtue of the iterative loop on `cp`.
fn parse_postfix(p: &mut Parser) {
    let cp = p.checkpoint();
    parse_primary(p);
    loop {
        // Postfix tokens must immediately follow without a trivia-crossing
        // newline-terminated semantic gap; we use raw `current()` to avoid
        // crossing INDENT/DEDENT (a property's RHS expression ends at the
        // mapping body's next NEWLINE).
        match p.current() {
            Some(SyntaxKind::DOT) => {
                // Treat `.IDENT` after a primary as POSTFIX member access.
                // (Disambiguation rule #2 in grammar.bnf line 252.)
                if p.peek_kind(1) != Some(SyntaxKind::IDENT) {
                    break;
                }
                p.start_at(cp, SyntaxKind::POSTFIX_EXPR);
                p.bump(); // DOT
                p.expect(SyntaxKind::IDENT);
                p.finish();
            }
            Some(SyntaxKind::LPAREN) => {
                p.start_at(cp, SyntaxKind::POSTFIX_EXPR);
                p.bump(); // LPAREN
                p.skip_trivia();
                if p.current() != Some(SyntaxKind::RPAREN) {
                    parse_arg_list(p);
                }
                p.expect(SyntaxKind::RPAREN);
                p.finish();
            }
            _ => break,
        }
    }
}

/// `ArgList := Arg (COMMA Arg)*` — grammar.bnf line 194.
fn parse_arg_list(p: &mut Parser) {
    p.start(SyntaxKind::ARG_LIST);
    parse_arg(p);
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::COMMA) {
            break;
        }
        p.bump(); // COMMA
        // Allow trailing comma followed by RPAREN — emit an empty ARG.
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::RPAREN) {
            break;
        }
        parse_arg(p);
    }
    p.finish();
}

/// `Arg := NamedArg | Expression` — grammar.bnf line 195.
/// `NamedArg := IDENT ASSIGN Expression` — line 196.
fn parse_arg(p: &mut Parser) {
    p.skip_trivia();
    let is_named =
        p.current() == Some(SyntaxKind::IDENT) && p.peek_kind(1) == Some(SyntaxKind::ASSIGN);
    if is_named {
        p.start(SyntaxKind::NAMED_ARG);
        p.bump(); // IDENT
        p.skip_trivia();
        p.bump(); // ASSIGN
    } else {
        p.start(SyntaxKind::ARG);
    }
    parse_expression(p, 0);
    p.finish();
}

/// `PrimaryExpr` per grammar.bnf line 199-211.
fn parse_primary(p: &mut Parser) {
    p.skip_trivia();
    match p.current() {
        // Literal-like primary tokens: numeric literals and strings. Both
        // wrap as LITERAL_EXPR with a single token payload.
        Some(SyntaxKind::INTEGER | SyntaxKind::FLOAT | SyntaxKind::STRING) => {
            p.start(SyntaxKind::LITERAL_EXPR);
            p.bump();
            p.finish();
        }
        Some(SyntaxKind::TEMPLATE) => {
            // A backtick literal with no hole in it. It carries no expression,
            // so there is nothing to carve and it stays one token.
            p.start(SyntaxKind::TEMPLATE_EXPR);
            p.bump();
            p.finish();
        }
        Some(SyntaxKind::STRING_OPEN) => parse_interpolated_string(p),
        Some(SyntaxKind::DOT) => {
            // FieldRef: `.IDENT (. IDENT)*` per grammar.bnf line 209.
            // At primary position, `.` always starts a FieldRef (disambig
            // rule #2: prior expression would have made `.IDENT` a postfix).
            p.start(SyntaxKind::FIELD_REF_EXPR);
            p.bump(); // DOT
            p.expect(SyntaxKind::IDENT);
            loop {
                if p.current() == Some(SyntaxKind::DOT) && p.peek_kind(1) == Some(SyntaxKind::IDENT)
                {
                    p.bump();
                    p.bump();
                } else {
                    break;
                }
            }
            p.finish();
        }
        Some(SyntaxKind::IDENT) => {
            // Disambig: `IDENT SHAPE_SEP IDENT` (no whitespace) = PrefixedName.
            // Per grammar.bnf line 226 the colon MUST be lexer-adjacent — no
            // whitespace between the IDENT and the `:`. Without this strict
            // contiguity check, `cond ? a : b` parses with `a : b` swallowed
            // as a prefixed-name IRI (disambig rule #3 violation).
            if p.peek_contiguous(3)
                && p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
                && p.peek_kind(2) == Some(SyntaxKind::IDENT)
            {
                p.start(SyntaxKind::IRI_EXPR);
                p.bump(); // IDENT
                p.bump(); // SHAPE_SEP
            } else {
                p.start(SyntaxKind::LITERAL_EXPR);
            }
            p.bump();
            p.finish();
        }
        Some(SyntaxKind::ABS_IRI) => {
            p.start(SyntaxKind::IRI_EXPR);
            p.bump();
            p.finish();
        }
        Some(SyntaxKind::LPAREN) => {
            p.start(SyntaxKind::PAREN_EXPR);
            p.bump(); // LPAREN
            parse_expression(p, 0);
            p.expect(SyntaxKind::RPAREN);
            p.finish();
        }
        Some(SyntaxKind::LBRACE) => parse_record_literal(p),
        _ => {
            // Recovery: emit ERROR with the current token (or empty if EOF).
            p.bump_as_error();
        }
    }
}

/// `InterpolatedString := STRING_OPEN (STRING_TEXT | Interpolation)*
/// STRING_CLOSE`, and `Interpolation := INTERP_OPEN Expression RBRACE`.
///
/// The hole holds an ordinary expression, read by the ordinary expression
/// parser. That is the whole content of ADR-0057's seventh amendment §3: there
/// is no format mini-language, so there is nothing that can drift out of step
/// with the checker.
fn parse_interpolated_string(p: &mut Parser) {
    p.start(SyntaxKind::INTERP_STRING_EXPR);
    p.bump(); // STRING_OPEN
    loop {
        match p.current() {
            Some(SyntaxKind::STRING_TEXT) => p.bump(),
            Some(SyntaxKind::INTERP_OPEN) => {
                p.start(SyntaxKind::INTERPOLATION);
                p.bump(); // INTERP_OPEN
                parse_interpolation_body(p);
                // The closing delimiter is the anchor, and it must survive: a
                // hole left unclosed should not also cost the string its end.
                crate::parser::recover::expect_or_recover(
                    p,
                    SyntaxKind::RBRACE,
                    &[SyntaxKind::STRING_CLOSE],
                );
                p.finish();
            }
            Some(SyntaxKind::STRING_CLOSE) => {
                p.bump();
                break;
            }
            // EOF, or a token the carve cannot have produced. Stop rather than
            // spin; the missing STRING_CLOSE is reported by `expect`.
            _ => {
                p.expect(SyntaxKind::STRING_CLOSE);
                break;
            }
        }
    }
    p.finish();
}

/// The body of one hole.
///
/// `${ex:}` — a prefix with no local part — is the commonest hole in today's
/// corpus (115 of 215) and is NOT an expression: `PrefixedName` requires a
/// local part. It is accepted HERE and nowhere else, because the backtick
/// spelling that produces it is retired by the seventh amendment and the
/// CURIE-with-holes goes with it: the replacement writes the IRI in full,
/// `"https://example.org/user/{u.id}"`. This arm dies with the last backtick
/// fixture — it is the one thing in this function that is not meant to last.
fn parse_interpolation_body(p: &mut Parser) {
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::IDENT)
        && p.peek_contiguous(2)
        && p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
        && p.peek_kind(2) == Some(SyntaxKind::RBRACE)
    {
        p.start(SyntaxKind::IRI_EXPR);
        p.bump(); // IDENT
        p.bump(); // SHAPE_SEP
        p.finish();
        return;
    }
    parse_expression(p, 0);
}

/// `RecordLiteral := LBRACE RecordBody RBRACE` (grammar.bnf line 232-236).
/// `RecordField := (IDENT | IRIExpr) ASSIGN Expression`.
fn parse_record_literal(p: &mut Parser) {
    p.start(SyntaxKind::RECORD_LITERAL);
    p.expect(SyntaxKind::LBRACE);
    loop {
        p.skip_trivia();
        match p.current() {
            None | Some(SyntaxKind::RBRACE) => break,
            // A field starts with IDENT (possibly followed by SHAPE_SEP IDENT
            // for a prefixed-name LHS) or an absolute IRI.
            Some(SyntaxKind::IDENT | SyntaxKind::ABS_IRI) => {
                p.start(SyntaxKind::RECORD_FIELD);
                // LHS — bare IDENT, prefixed name (lexer-contiguous), or ABS_IRI.
                if p.current() == Some(SyntaxKind::IDENT)
                    && p.peek_contiguous(3)
                    && p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
                    && p.peek_kind(2) == Some(SyntaxKind::IDENT)
                {
                    p.start(SyntaxKind::IRI_EXPR);
                    p.bump();
                    p.bump();
                    p.bump();
                    p.finish();
                } else {
                    p.bump();
                }
                p.expect(SyntaxKind::ASSIGN);
                parse_expression(p, 0);
                p.finish(); // RECORD_FIELD
                // RecordSep := COMMA | NEWLINE (grammar.bnf line 235).
                p.skip_trivia();
                if p.current() == Some(SyntaxKind::COMMA) {
                    p.bump();
                }
            }
            _ => {
                p.bump_as_error();
                break;
            }
        }
    }
    p.expect(SyntaxKind::RBRACE);
    p.finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indent::lex_with_indents;
    use crate::kind::SyntaxNode;
    use rowan::GreenNode;

    /// Parse a snippet as a single expression (no surrounding item context)
    /// and return the resulting root node for shape assertion.
    fn parse_expr_node(src: &str) -> SyntaxNode {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        p.start(SyntaxKind::EXPR);
        parse_expression(&mut p, 0);
        p.finish();
        let green: GreenNode = p.builder.finish();
        SyntaxNode::new_root(green)
    }

    /// Collect the non-trivia child kinds of a node into a flat list, useful
    /// for shape assertions.
    fn child_kinds(n: &SyntaxNode) -> Vec<SyntaxKind> {
        n.children().map(|c| c.kind()).collect()
    }

    #[test]
    fn pratt_pipe_is_left_assoc() {
        // a |> f |> g  →  PIPELINE_EXPR( PIPELINE_EXPR(a, f), g )
        let node = parse_expr_node("a |> f |> g");
        let outer = node.first_child().expect("an expression child");
        assert_eq!(outer.kind(), SyntaxKind::PIPELINE_EXPR);
        let kids = child_kinds(&outer);
        assert_eq!(
            kids,
            vec![SyntaxKind::PIPELINE_EXPR, SyntaxKind::LITERAL_EXPR]
        );
    }

    #[test]
    fn pratt_ternary_is_right_assoc() {
        // c1 ? a : c2 ? b : d  →  TERNARY( c1, a, TERNARY( c2, b, d ) )
        let node = parse_expr_node("c1 ? a : c2 ? b : d");
        let outer = node.first_child().expect("an expression child");
        assert_eq!(outer.kind(), SyntaxKind::TERNARY_EXPR);
        // The third child (else-branch) should itself be a TERNARY_EXPR.
        let inner_else = outer.children().nth(2).expect("an else-branch child node");
        assert_eq!(inner_else.kind(), SyntaxKind::TERNARY_EXPR);
    }

    #[test]
    fn pratt_precedence_walk() {
        // a or b and c == d + e * f
        //   →  BIN(or, a, BIN(and, b, BIN(==, c, BIN(+, d, BIN(*, e, f)))))
        let node = parse_expr_node("a or b and c == d + e * f");
        let outer = node.first_child().expect("an expression child");
        assert_eq!(outer.kind(), SyntaxKind::BINARY_EXPR);
        // Walk: each right child should also be BINARY_EXPR until we reach
        // the innermost `e * f`.
        let mut here = outer;
        for _ in 0..4 {
            here = here.children().nth(1).expect("a right child");
            assert_eq!(here.kind(), SyntaxKind::BINARY_EXPR);
        }
        // Now `here` is the innermost BINARY_EXPR (* between e and f). Its
        // children are both LITERAL_EXPR.
        let kids = child_kinds(&here);
        assert_eq!(
            kids,
            vec![SyntaxKind::LITERAL_EXPR, SyntaxKind::LITERAL_EXPR]
        );
    }

    #[test]
    fn pratt_unary_minus_right_assoc() {
        // - - x  →  UNARY( UNARY( x ) )
        let node = parse_expr_node("- - x");
        let outer = node.first_child().expect("an expression child");
        assert_eq!(outer.kind(), SyntaxKind::UNARY_EXPR);
        let inner = outer
            .children()
            .next()
            .expect("inner unary expression child");
        assert_eq!(inner.kind(), SyntaxKind::UNARY_EXPR);
    }

    #[test]
    fn pratt_postfix_chain() {
        // f(x).y(z)  →  POSTFIX( POSTFIX( POSTFIX( f, (x) ), .y ), (z) )
        let node = parse_expr_node("f(x).y(z)");
        let outer = node.first_child().expect("an expression child");
        assert_eq!(outer.kind(), SyntaxKind::POSTFIX_EXPR);
        // Walk down: outer is the (z) call; its first child is the .y access;
        // whose first child is the (x) call; whose first child is the bare `f`.
        let z_call_inner = outer.first_child().expect("inner POSTFIX child");
        assert_eq!(z_call_inner.kind(), SyntaxKind::POSTFIX_EXPR);
        let y_access_inner = z_call_inner.first_child().expect("inner POSTFIX child");
        assert_eq!(y_access_inner.kind(), SyntaxKind::POSTFIX_EXPR);
        let f_ident = y_access_inner.first_child().expect("primary at base");
        assert_eq!(f_ident.kind(), SyntaxKind::LITERAL_EXPR);
    }

    #[test]
    fn pratt_comp_non_assoc_chain_does_not_nest() {
        // `a < b < c` — the second `<` MUST NOT nest inside the first; it
        // should bail out leaving the trailing `< c` as siblings of the
        // outer BINARY_EXPR. The outer BINARY_EXPR contains exactly two
        // children (a, b); after the loop bails, the parent EXPR contains
        // additional sibling tokens.
        let node = parse_expr_node("a < b < c");
        let outer = node.first_child().expect("first expression child");
        assert_eq!(outer.kind(), SyntaxKind::BINARY_EXPR);
        let kids = child_kinds(&outer);
        assert_eq!(
            kids,
            vec![SyntaxKind::LITERAL_EXPR, SyntaxKind::LITERAL_EXPR],
            "non-assoc comparison must not chain"
        );
    }
}
