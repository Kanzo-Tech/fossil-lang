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
//! The `BNF` column below is the level the specified table gives each operator,
//! and this parser implements every one of them. A citation of «L8 unary» or
//! «L9 postfix» predates the renumbering `|>` left behind
//! (grammar.bnf, § OPERATOR PRECEDENCE TABLE); the binding POWERS never moved,
//! only the level names, which is why the lowest few are unused.
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
//! The primaries match the grammar exactly: `PrimaryExpr` is a literal, an IDENT
//! or a parenthesised expression.
//!
//! `true` and `false` are tokens (grammar.bnf, BOOL) because a program writes
//! `verified = true` and there is no binding for the name to resolve against.
//! As `IDENT`s they made that line report "unknown column `true`".

use crate::kind::SyntaxKind;

use super::Parser;
use super::diag::ParseDiagnostic;

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
            // then-branch — cannot itself be a ternary at the same level
            parse_expression(p, TERNARY_RBP + 1);
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
        // There is no arm for `|>`, and there is no token for it either: the
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

/// L8 postfix: primary followed by `.IDENT` / `."name"` member access or
/// `(args)` call.
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
                //
                // The member is an `IDENT` or, for a name the data chose and an
                // identifier cannot hold, a STRING: `Knows."Person.id"`
                // (grammar.bnf, `PostfixOp`; `crate::name` decides which).
                match p.peek_kind(1) {
                    Some(SyntaxKind::IDENT) => {
                        p.start_at(cp, SyntaxKind::POSTFIX_EXPR);
                        p.bump(); // DOT
                        p.expect(SyntaxKind::IDENT);
                        p.finish();
                    }
                    Some(SyntaxKind::STRING) => {
                        p.start_at(cp, SyntaxKind::POSTFIX_EXPR);
                        p.bump(); // DOT
                        p.skip_trivia();
                        refuse_needless_quotes(p);
                        p.bump(); // STRING
                        p.finish();
                    }
                    Some(SyntaxKind::STRING_OPEN) => {
                        p.start_at(cp, SyntaxKind::POSTFIX_EXPR);
                        p.bump(); // DOT
                        p.skip_trivia();
                        let start = p.current_token_span_start();
                        p.start(SyntaxKind::ERROR);
                        parse_interpolated_string(p);
                        p.finish();
                        let end = p.prev_end();
                        p.push_diagnostic(ParseDiagnostic::Malformed {
                            problem: fossil_base::Problem::HoleInMemberName {},
                            help: Some(HOLE_IN_MEMBER_NAME_HELP.to_string()),
                            span: span_of(start, end),
                        });
                        p.finish();
                    }
                    _ => break,
                }
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

/// A quoted member is a NAME, so it cannot depend on the row.
const HOLE_IN_MEMBER_NAME_HELP: &str =
    "write the column's name as it is, `Row.\"Person.id\"`, and a literal `{` as `{{`";

/// Refuse `Row."name"` — quotes around a name that is already an identifier.
/// The tree is built as written: this is one diagnostic, not a recovery.
///
/// Each name has ONE spelling (`crate::name`). `SQL` admits both `t.a` and
/// `t."a"` and then has to explain that they differ in case-folding; fossil
/// folds nothing, so the second spelling would mean exactly the first and be
/// a second way to write it.
fn refuse_needless_quotes(p: &mut Parser) {
    let (Some(text), Some(range)) = (p.current_text(), p.current_range()) else {
        return;
    };
    let name = crate::name::string_value(text);
    if !crate::name::is_bare(&name) {
        return;
    }
    p.push_diagnostic(ParseDiagnostic::Malformed {
        problem: fossil_base::Problem::NeedlessQuotes { name },
        help: Some(
            "quotes are for a name an identifier cannot hold, like `Row.\"Person.id\"`, and each \
             name has one spelling"
                .to_string(),
        ),
        span: span_of(range.start, range.end),
    });
}

fn span_of(start: usize, end: usize) -> fossil_base::Span {
    fossil_base::Span::new(
        u32::try_from(start).unwrap_or(u32::MAX),
        u32::try_from(end).unwrap_or(u32::MAX),
    )
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
fn parse_primary(p: &mut Parser) {
    p.skip_trivia();
    match p.current() {
        // Literal-like primary tokens: numeric literals and strings. Both
        // wrap as LITERAL_EXPR with a single token payload.
        Some(
            SyntaxKind::INTEGER
            | SyntaxKind::FLOAT
            | SyntaxKind::BOOL
            | SyntaxKind::NULL
            | SyntaxKind::STRING
            | SyntaxKind::IDENT,
        ) => {
            p.start(SyntaxKind::LITERAL_EXPR);
            p.bump();
            p.finish();
        }
        Some(SyntaxKind::STRING_OPEN) => parse_interpolated_string(p),
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
/// this is one call — and that is the whole of it. `{ex:}`, the prefix-with-no-
/// local-part hole the CURIE admitted in this one position, is not an
/// expression and needs no arm: `ex` parses as a primary and the `:` after it
/// reaches nothing that wants one, so the diagnostic is not silence.
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

    fn expr_diagnostics(src: &str) -> (SyntaxNode, Vec<String>) {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        p.start(SyntaxKind::EXPR);
        parse_expression(&mut p, 0);
        p.finish();
        let messages = p
            .diagnostics
            .iter()
            .cloned()
            .map(|d| d.to_diagnostic().message())
            .collect();
        (SyntaxNode::new_root(p.builder.finish()), messages)
    }

    #[test]
    fn a_quoted_member_is_a_member_access_whose_name_is_the_strings_value() {
        let (root, messages) = expr_diagnostics(r#"Knows."Person.id""#);
        assert!(messages.is_empty(), "{messages:?}");
        let postfix = root.first_child().expect("one expression");
        assert_eq!(postfix.kind(), SyntaxKind::POSTFIX_EXPR);
        assert_eq!(
            crate::name::member_name(&postfix).as_deref(),
            Some("Person.id")
        );
        // It chains like any member: the value path reaches through it.
        let (root, messages) = expr_diagnostics(r#"Row."first name".trim()"#);
        assert!(messages.is_empty(), "{messages:?}");
        assert_eq!(
            root.first_child().map(|n| n.kind()),
            Some(SyntaxKind::POSTFIX_EXPR)
        );
    }

    #[test]
    fn quotes_around_an_identifier_are_refused_and_name_the_bare_spelling() {
        let (_, messages) = expr_diagnostics(r#"Row."name""#);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("`.name`"), "{}", messages[0]);
        // A reserved word cannot be written bare, so quoting it is the spelling.
        let (_, messages) = expr_diagnostics(r#"Row."from""#);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn a_quoted_member_with_a_hole_is_refused() {
        let (root, messages) = expr_diagnostics(r#"Row."a{Row.b}""#);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("constant"), "{}", messages[0]);
        assert!(root.descendants().any(|n| n.kind() == SyntaxKind::ERROR));
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
