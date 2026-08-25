// The parent `parser` module declares this submodule `pub(crate) mod expr;`
// (private to the crate). Items inside that need to be reachable from sibling
// submodules (e.g. `parser::items` calls `parse_expression`) must be
// `pub(crate)` to satisfy the `unreachable_pub` lint; that combo trips the
// inverse `redundant_pub_crate` lint, which we silence here. The visibility
// IS correct — both lints can't be satisfied simultaneously for a sibling-
// callable item in a `pub(crate)` submodule.
#![allow(clippy::redundant_pub_crate)]

//! Pratt expression sub-parser for the specified precedence table
//! (grammar.bnf, § OPERATOR PRECEDENCE TABLE).
//!
//! Entry point: [`parse_expression`].
//!
//! Reference: Crafting Interpreters Ch. 17 (Nystrom) + rust-analyzer's
//! `crates/parser/src/grammar/expressions.rs`. The Pratt shape is materially
//! cleaner than nested precedence functions.
//!
//! The specified table has EIGHT levels and this parser now implements exactly
//! those eight. `|>` held a ninth at the tightest-binding end of the low side
//! until the pipeline was retired for being a second spelling of `a.f()`, and
//! every level moved up one when it went
//! (grammar.bnf, § OPERATOR PRECEDENCE TABLE). A citation of «L8 unary» or
//! «L9 postfix» predates that renumbering.
//!
//! The binding POWERS below did not change with the renumbering — only the
//! level names did — so `1` and `2` are simply unused now.
//!
//! Binding-power table (lbp = left binding power; rbp = right binding
//! power; tighter operators → higher numbers). The `BNF` column is the
//! grammar's level:
//!
//! | BNF | Op                     | Assoc       | lbp | rbp |
//! |-----|------------------------|-------------|-----|-----|
//! | L1  | `? :`                  | right       |   4 |   3 |
//! | L2  | `or`                   | left        |   5 |   6 |
//! | L3  | `and`                  | left        |   7 |   8 |
//! | L4  | `== != < <= > >=`      | non-assoc   |   9 |  10 |
//! | L5  | `+ -`                  | left        |  11 |  12 |
//! | L6  | `* / %`                | left        |  13 |  14 |
//! | L7  | unary `-`, `not`       | right       |  —  |  15 |
//! | L8  | `.` member, `()` call  | left        |  17 |  —  |
//!
//! Left-associative operators give `rbp = lbp + 1`; right-associative
//! operators give `rbp = lbp - 1`. Non-associative operators check that
//! the LHS was not already produced at the same level and bail with an
//! ERROR otherwise.
//!
//! The primaries now match the grammar exactly: `PrimaryExpr` is a literal, an
//! IDENT or a parenthesised expression. [`parse_primary`] used to read four
//! more — the leading-dot `FieldRef`, the CURIE `ex:name`, the `<…>` absolute
//! IRI and the backtick `TEMPLATE` — and each of the four now has a REFUSAL arm
//! in its place rather than nothing at all, because the parser can still see
//! what was written and a bare `unexpected token` throws that away.
//!
//! `BOOL` was the one terminal still missing and is missing no longer: `true`
//! and `false` are specified as tokens (grammar.bnf, BOOL) because a program
//! writes `verified = true` and there is no binding for the name to resolve
//! against. They arrived as `IDENT` until the lexer had a rule for them, which
//! made `verified = true` report "unknown column `true`".

use crate::kind::SyntaxKind;

use super::diag::retired;
use super::{Parser, RetiredRun};

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
    /// Non-associative (the comparison operators — L4 in grammar.bnf).
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
            // The then-branch is parsed with the ternary marked open, so
            // `parse_primary` knows the `:` waiting past it is this ternary's
            // and not a retired CURIE. See `Parser::ternary_then_depth`.
            p.inside_ternary_then(|p| {
                // then-branch — cannot itself be a ternary at the same level
                parse_expression(p, TERNARY_RBP + 1);
            });
            p.skip_trivia();
            // Disambiguation rule #3: the `:` here is a `SHAPE_SEP`, the same
            // token a mapping header takes, paired with the just-consumed `?`.
            // The grammar used to declare a separate `T_COLON` terminal for
            // this position that the lexer never emitted.
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

        // `a |> f()` — the retired pipeline; `|>` is not a token. With the
        // lexer rule gone, `|>` arrives as the unlexable `|` (an ERROR token
        // carrying its text) followed by `GT`, so the two are matched here
        // rather than by a kind. This is infix position and an operand is
        // already parsed, which is the only place the operator could stand.
        if p.current() == Some(SyntaxKind::ERROR)
            && p.current_text() == Some("|")
            && p.peek_kind(1) == Some(SyntaxKind::GT)
        {
            p.retire(retired::PIPELINE, RetiredRun::Count(2));
            break;
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
        // There is no arm for `|>`, and there is no token for it either. It held
        // L1; retiring it moved every level up one
        // (grammar.bnf, § OPERATOR PRECEDENCE TABLE). The binding powers below
        // did not change — only the level NAMES did, so `or` is L2 where it used
        // to be called L3. The
        // refusal is in `parse_expression`, in infix position, because that is
        // the only place `|>` could ever stand.
        // L2
        SyntaxKind::KW_OR => (5, 6, Assoc::Assoc, SyntaxKind::BINARY_EXPR),
        // L3
        SyntaxKind::KW_AND => (7, 8, Assoc::Assoc, SyntaxKind::BINARY_EXPR),
        // L4 (non-associative)
        SyntaxKind::EQ
        | SyntaxKind::NEQ
        | SyntaxKind::LT
        | SyntaxKind::LE
        | SyntaxKind::GT
        | SyntaxKind::GE => (9, 10, Assoc::Non, SyntaxKind::BINARY_EXPR),
        // L5
        SyntaxKind::PLUS | SyntaxKind::MINUS => (11, 12, Assoc::Assoc, SyntaxKind::BINARY_EXPR),
        // L6
        SyntaxKind::STAR | SyntaxKind::SLASH | SyntaxKind::PERCENT => {
            (13, 14, Assoc::Assoc, SyntaxKind::BINARY_EXPR)
        }
        _ => return None,
    })
}

/// L7 unary, or fall through to L8 primary + postfix.
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

/// L8 postfix: primary followed by `.IDENT` member access or `(args)` call.
/// Left-associative by virtue of the iterative loop on `cp`.
///
/// `.` is the ONLY access operator in the grammar and it means «member of»;
/// what is on the left decides what the members are. The verbs
/// are not productions — `where`, `select` and `join` are catalogue entries
/// resolved by receiver type, so a new verb is a row rather than a rule, and
/// this loop needs to know nothing about any of them.
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
                // `.IDENT` after a primary is POSTFIX member access — the only
                // reading the grammar leaves a `.` (its disambiguation rule 2,
                // which asked whether an expression preceded the dot, went with
                // the FieldRef that made it a question).
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

/// `ArgList := Arg (COMMA Arg)*`, as specified.
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

/// `Arg := NamedArg | AliasArg | Expression` (grammar.bnf, Arg), with
/// `NamedArg := IDENT ASSIGN Expression` (grammar.bnf, `NamedArg`) and
/// `AliasArg := IDENT 'as' IDENT` (grammar.bnf, `AliasArg`).
///
/// # The three forks, and why one token of lookahead is enough for all of them
///
/// All three start with an operand and the second token separates them:
/// `=` opens a named argument, a bare `IDENT` opens an alias, and anything else
/// is an ordinary expression. The alias is the fork worth arguing: `as` is
/// CONTEXTUAL, not reserved (rule 7; grammar.bnf, § DISAMBIGUATION RULES), so
/// it is recognised only where an operand is followed by a bare `IDENT` — and
/// no expression can
/// continue that way, because this language has no juxtaposition. Everywhere
/// else `as` is an ordinary identifier, and a column called `as` still parses.
///
/// `AliasArg` is the self-join — `Node.join(Node as Other, on = Node.parent ==
/// Other.id)`. It binds a second name for the same source so the two sides can
/// be told apart, and the mapping body then writes `Other.label` next to
/// `Node.label`.
fn parse_arg(p: &mut Parser) {
    p.skip_trivia();
    let second = p.peek_kind(1);
    let is_named = p.current() == Some(SyntaxKind::IDENT) && second == Some(SyntaxKind::ASSIGN);
    // `IDENT IDENT`, where the second one is the word `as`. The text check is
    // what keeps `as` an identifier: `Node other` is still two tokens nothing
    // claims, and only `Node as Other` is an alias.
    let is_alias = p.current() == Some(SyntaxKind::IDENT)
        && second == Some(SyntaxKind::IDENT)
        && p.peek_text(1) == Some("as")
        && p.peek_kind(2) == Some(SyntaxKind::IDENT);
    if is_named {
        p.start(SyntaxKind::NAMED_ARG);
        p.bump(); // IDENT
        p.skip_trivia();
        p.bump(); // ASSIGN
    } else if is_alias {
        p.start(SyntaxKind::ALIAS_ARG);
        p.bump(); // IDENT — the source being aliased
        p.skip_trivia();
        p.bump(); // IDENT `as`
        p.skip_trivia();
        p.bump(); // IDENT — the alias
        p.finish();
        return;
    } else {
        p.start(SyntaxKind::ARG);
    }
    parse_expression(p, 0);
    p.finish();
}

/// `PrimaryExpr := Literal | IDENT | LPAREN Expression RPAREN`
/// (grammar.bnf, `PrimaryExpr`), where `Literal` is `INTEGER | FLOAT | STRING |
/// BOOL | InterpolatedString` (grammar.bnf, Literal).
///
/// Three alternatives, as specified, plus three REFUSALS. The refusals are not
/// alternatives: each consumes a retired spelling under an `ERROR` node so the
/// message can name what replaces it. Falling through to `bump_as_error`
/// instead would report `unexpected token` for a form the parser recognised
/// exactly, and this file's whole failure history is silence in that position.
fn parse_primary(p: &mut Parser) {
    p.skip_trivia();
    // The literal arm and the bare-`IDENT` arm have the same body and cannot be
    // merged: the guarded CURIE refusal sits between them, and match arms are
    // tried in order. Folding `IDENT` into the literal arm above would shadow
    // that guard, and `ex:name` would parse as a literal instead of naming what
    // replaced it.
    #[allow(clippy::match_same_arms)]
    match p.current() {
        // Literal-like primary tokens: numeric literals and strings. Both
        // wrap as LITERAL_EXPR with a single token payload.
        Some(
            SyntaxKind::INTEGER
            | SyntaxKind::FLOAT
            | SyntaxKind::BOOL
            | SyntaxKind::NULL
            | SyntaxKind::STRING,
        ) => {
            p.start(SyntaxKind::LITERAL_EXPR);
            p.bump();
            p.finish();
        }
        Some(SyntaxKind::STRING_OPEN) => parse_interpolated_string(p),
        // `.name` — the leading-dot `FieldRef`, and a leading `.` is an error
        // now. It named a column of an anonymous current row, and the row has a
        // name: every reference is qualified, `User.name`. The
        // run is the `.` and the name after it, so the span underlines the
        // reference rather than the dot alone.
        Some(SyntaxKind::DOT) => {
            let run = if p.peek_kind(1) == Some(SyntaxKind::IDENT) {
                RetiredRun::Count(2)
            } else {
                RetiredRun::Count(1)
            };
            p.retire(retired::LEADING_DOT, run);
        }
        // `ex:name` — the CURIE, and a `:` that is not a mapping header or a
        // ternary is an error now. The whitespace check is
        // gone; what stands in its place is `in_ternary_then`, which is a fact
        // the parser already has rather than a fact about the source's spacing.
        // Inside a then-branch the `:` is the ternary's, so `cond ? a:b` parses
        // clean — the very case the deleted rule got wrong.
        Some(SyntaxKind::IDENT)
            if !p.in_ternary_then()
                && p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
                && p.peek_kind(2) == Some(SyntaxKind::IDENT) =>
        {
            p.retire(retired::CURIE, RetiredRun::Count(3));
        }
        Some(SyntaxKind::IDENT) => {
            p.start(SyntaxKind::LITERAL_EXPR);
            p.bump();
            p.finish();
        }
        // `<http://…>` — no longer one token, so what arrives is `LT` and the
        // operands after it. A constant IRI is a STRING and the shape decides
        // that it denotes rather than reads.
        //
        // This arm CANNOT steal a comparison: `a < b` reaches `peek_infix` with
        // `a` already parsed, and `parse_primary` is only ever entered where an
        // operand is due. A `<` in operand position was always the IRI.
        Some(SyntaxKind::LT) => {
            p.retire(retired::ABSOLUTE_IRI, RetiredRun::Line);
        }
        Some(SyntaxKind::LPAREN) => {
            p.start(SyntaxKind::PAREN_EXPR);
            p.bump(); // LPAREN
            parse_expression(p, 0);
            p.expect(SyntaxKind::RPAREN);
            p.finish();
        }
        // `{` used to open a RecordLiteral here — the blank node in value
        // position. It went with the annotation block it shared a brace with,
        // so a `{` in an expression falls to the recovery arm like any other
        // token nothing claims.
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
/// parser, and that is the whole of it: there is no format mini-language, so
/// there is nothing that can drift out of step
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
/// It takes an EXPRESSION and nothing else (grammar.bnf, Interpolation), so
/// this is one call — and that is the whole of it. There was a special arm for
/// `${ex:}`, a prefix with no local part, which was the commonest hole in the
/// old corpus (115 of 215) and was NOT an expression at all: `PrefixedName`
/// requires a local part, so the form was admitted in this one position and
/// nowhere else. It died with the CURIE, and a full IRI is written out —
/// `"https://shop.example/user/{User.email}"`.
///
/// `{ex:}` still produces a diagnostic rather than silence: `ex` parses as a
/// primary and the `:` after it reaches nothing that wants one.
fn parse_interpolation_body(p: &mut Parser) {
    p.skip_trivia();
    parse_expression(p, 0);
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

    /// `|>` is refused BY NAME, not as an unexpected token.
    ///
    /// The operand before it still parses — the refusal happens in infix
    /// position, so `a` is a `LITERAL_EXPR` and only the `|>` is consumed under
    /// the `ERROR` node. That is the difference between telling the author what
    /// replaces the form and telling them a byte surprised the parser.
    #[test]
    fn pipe_is_retired_with_a_message_naming_the_member_call() {
        let tokens = lex_with_indents("a |> f()");
        let mut p = Parser::new(tokens);
        p.start(SyntaxKind::EXPR);
        parse_expression(&mut p, 0);
        p.finish();
        let retired: Vec<_> = p
            .diagnostics
            .iter()
            .filter_map(|d| match d {
                crate::parser::diag::ParseDiagnostic::RetiredSpelling { message, span } => {
                    Some((message.clone(), *span))
                }
                _ => None,
            })
            .collect();
        assert_eq!(retired.len(), 1, "exactly one refusal, got {retired:?}");
        let (message, span) = &retired[0];
        assert!(
            message.contains("a.f("),
            "the message must name what replaces `|>`: {message}"
        );
        // The span underlines the two bytes of `|>` and nothing else.
        assert_eq!((span.start, span.end), (2, 4), "span over `|>`");
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
    fn a_boolean_literal_is_a_literal_and_not_a_name() {
        // The distinction the BOOL token exists to make. As an `IDENT`, `true`
        // is a `LITERAL_EXPR` too — the shape is identical — so the assertion
        // that carries the meaning is on the TOKEN kind inside it.
        for src in ["true", "false"] {
            let node = parse_expr_node(src);
            let lit = node.first_child().expect("an expression child");
            assert_eq!(lit.kind(), SyntaxKind::LITERAL_EXPR, "{src}");
            let tok = lit
                .children_with_tokens()
                .filter_map(crate::kind::SyntaxElement::into_token)
                .find(|t| !matches!(t.kind(), SyntaxKind::WHITESPACE))
                .expect("one token");
            assert_eq!(tok.kind(), SyntaxKind::BOOL, "`{src}` must be a BOOL token");
            assert_eq!(tok.text(), src);
        }
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
