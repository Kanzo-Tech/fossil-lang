// See `parser::expr` for the rationale on the redundant_pub_crate allow.
// `parser::items::parse_program` is called from `parser::Parser::parse_program`
// (the sibling), so the item needs `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

//! Item-level recursive-descent parser.
//!
//! Items are LL(k) on a fixed, tiny k — one token of lookahead everywhere but
//! rule 8, which needs the token after `:=` — so no backtracking is needed. We
//! keep recursive descent here and delegate every expression slot to the Pratt
//! sub-parser in [`super::expr::parse_expression`].
//!
//! # What this module parses, against what `grammar.bnf` specifies
//!
//! ```text
//! Program             := TopLevel* EOF                     — as specified
//! TopLevel            := SourceDef | MultiSourceDef
//!                      | TypeDef | PolicyDef | Mapping     — as specified
//! TypeDef             := RenameAttr* 'type' LBRACE IDENT (COMMA IDENT)* RBRACE
//!                        DEFINE Expression                 — as specified
//! PolicyDef           := 'policy' DEFINE STRING            — as specified
//! RenameAttr          := AT_ATTR LPAREN IDENT (COMMA Rename)+ RPAREN
//!                                                          — as specified
//! Rename              := STRING 'as' IDENT                 — as specified
//! Mapping             := MappingHeader NEWLINE INDENT MappingBody DEDENT
//! MappingHeader       := IDENT SHAPE_SEP ShapeExpr 'from' Expression
//! ShapeExpr           := IDENT                             — as specified
//! MappingBody         := (SubjectAssign | Property)*
//! SubjectAssign       := AT_ATTR ASSIGN Expression          — as specified
//! Property            := PropertyLhs ASSIGN Expression      — as specified
//! PropertyLhs         := IDENT                              — as specified
//! ```
//!
//! One production differs, and it is a deliberate division of labour rather
//! than work outstanding: **`MappingBody := SubjectAssign Property+`** — the
//! identity is required, there is exactly one, and it is the first line.
//! [`parse_mapping_body`] requires none of the three; `fossil_hir::body::body`
//! checks all three, because each is a fact about a mapping rather than about a
//! token, and the message wants the mapping's name in it.
//!
//! `SourceDef` and `MultiSourceDef` match the grammar. `def_map` scans for
//! `SOURCE_DEF`, which is what the walking-skeleton invariant rests on.
//!
//! There is no `parse_import` and no `parse_prefix_decl`, and the grammar has
//! neither production: a file is compiled alone, so there is no module system
//! for a name to come from, and a vocabulary declaration that nothing spells is
//! a statement about nothing. `use` and `prefix` are ordinary identifiers.
//!
//! # The retired spellings are REFUSED here, not recovered past
//!
//! Three of the six live in this module — the vocabulary declaration
//! ([`parse_program`]), the CURIE in a shape or a property key
//! ([`parse_shape_expr`], [`parse_property_lhs`]) and the `<…>` absolute IRI
//! ([`parse_property_lhs`]). Each CONSUMES its form under an `ERROR` node and
//! reports [`super::diag::retired`]'s message for it, because leaving the
//! tokens to `recover_to` produces `unexpected token` for something the parser
//! recognised exactly.
//!
//! # Disambiguation rules (grammar.bnf, § DISAMBIGUATION RULES)
//!
//! The grammar numbers five, having retired three. What this module and
//! `super::expr` implement, exercised by `mod disambiguation` at the bottom of
//! this file:
//!
//! (Rule 1 was `RecordLiteral` vs `AnnotationBlock` — whether the `{` came
//! straight after `=` or after an expression. Both forms went, and telling
//! two absent things apart is not a rule. A `{` in a mapping body is now
//! one thing: an error.)
//! (Rule 2 was `.IDENT` `FieldRef` vs `expr.IDENT` postfix member access. Every
//! reference is qualified now, `User.name`, so a leading `.` starts nothing and
//! there is no fork: `super::expr::parse_primary` refuses it by name.)
//! 3. `:` in a `MappingHeader` vs `:` in a ternary: one lexeme, one token
//!    (`SHAPE_SEP`), consumed by [`parse_mapping_header`] in the first case
//!    and by [`super::expr::parse_expression`]'s ternary handler in the
//!    second. The node is what tells them apart, not the token. The third
//!    reading — the CURIE — is gone, and so is the contiguity check that
//!    existed only to keep `a : b` out of a ternary.
//! (Rule 4 was `<<` vs `<`, and it is gone with the triple term. With `ABS_IRI`
//! gone, `<` no longer even opens.)
//! 5. `type` at top level: `type {` is a `TypeDef`, `type :=` a `SourceDef`
//!    binding the name `type`, `type :` a mapping called `type`. One token of
//!    lookahead, in [`parse_program`], and the word stays a column name.
//! 6. `@rename` vs `@subject`: both lex as `AT_ATTR` and POSITION decides.
//!    [`parse_rename_attr`] takes the top-level position — reached because
//!    `TypeDef := RenameAttr* 'type' …` puts `AT_ATTR` in that production's
//!    FIRST set — and [`parse_subject_assign`] takes the body position, which
//!    is reachable only through an `INDENT`. Each checks the NAME and refuses
//!    the other's, so neither is valid in the other's position and no third
//!    name is valid anywhere.
//! 7. `as`, contextual in `STRING as IDENT` (inside `@rename`, [`parse_rename`])
//!    and `IDENT as IDENT` (the self-join alias,
//!    `super::expr::parse_arg`). One token of lookahead settles both: an
//!    operand followed by a bare `IDENT`, which no expression can continue
//!    because the language has no juxtaposition. Everywhere else `as` is an
//!    ordinary identifier and lexes as one.
//! 8. `policy` at top level: `policy := STRING` is a `PolicyDef`, `policy :=`
//!    anything else a `SourceDef` binding the name `policy`, `policy :` a
//!    mapping called `policy`. TWO tokens of lookahead past the name, in
//!    [`parse_program`], and it is two rather than rule 5's one because the
//!    RIGHT-hand side decides — which is what leaves `policy` an ordinary
//!    identifier in every position, where rule 5 costs `type` the `type {`
//!    opening.

use crate::kind::SyntaxKind;

use super::diag::retired;
use super::recover::{self, MAPPING_BODY_ANCHORS, TOP_LEVEL_ANCHORS};
use super::{Parser, RetiredRun};

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
            // `@rename(Person, "…" as foaf_name)` above a `type` binding.
            //
            // `TypeDef := RenameAttr* 'type' …` (grammar.bnf, TypeDef), so this
            // is not a `TopLevel` alternative of its own: `TopLevel := … |
            // TypeDef | …` already derives it, and what a `RenameAttr*` prefix
            // changed is the FIRST set — `TypeDef` now starts with `AT_ATTR` as
            // well as with the contextual `type`. Hence two entries into one
            // function rather than a fifth item.
            //
            // LL(1) survives it because `AT_ATTR` has exactly ONE derivation
            // here: `SubjectAssign` lives inside a mapping body, which is
            // reached only through `INDENT`. That is disambiguation rule 6
            // (grammar.bnf, § DISAMBIGUATION RULES) — both names lex as
            // `AT_ATTR` and POSITION decides — and `parse_rename_attr` is where
            // the position is
            // enforced, by name, in both directions.
            Some(SyntaxKind::AT_ATTR) => parse_type_def(p),
            // `{ A, B, ... } := io.rdf(...)` — a destructuring source def. A
            // top-level `{` is the only `{` there is now: the selective-import
            // brace went with `use`, and the record and annotation braces went
            // with the forms that opened them.
            Some(SyntaxKind::LBRACE) => parse_multi_source_def(p),
            Some(SyntaxKind::IDENT) => match p.peek_kind(1) {
                // `policy := "people.jsonld"` → the release's privacy policy
                // (POLICY_DEF). TWO tokens of lookahead past the name, and the
                // second one is what makes this cost nothing: the RHS decides,
                // so `policy := io.csv("p.csv")` falls through to the arm below
                // and still binds a source called `policy`. That is
                // disambiguation rule 8 (grammar.bnf, § DISAMBIGUATION RULES),
                // and it is stricter than rule 5 — `type` loses the `type {`
                // opening, `policy` loses nothing.
                //
                // `SyntaxKind::STRING` and not `STRING_OPEN`: an interpolated
                // reference has nothing in scope to interpolate, and the bound
                // has to be readable off the source text without running the
                // program. `policy := "p-{x}.jsonld"` is a SOURCE_DEF and the
                // checker refuses it as one.
                Some(SyntaxKind::DEFINE)
                    if p.current_text() == Some("policy")
                        && p.peek_kind(2) == Some(SyntaxKind::STRING) =>
                {
                    parse_policy_def(p);
                }
                // `IDENT :=` → source binding (SOURCE_DEF).
                Some(SyntaxKind::DEFINE) => parse_source_def(p),
                // `IDENT :` → start of a mapping header.
                Some(SyntaxKind::SHAPE_SEP) => parse_mapping(p),
                // `type { A, B } := …` → a type binding (TYPE_DEF). `type` is a
                // CONTEXTUAL keyword, not a reserved word: the slot `IDENT {`
                // was free at top level, and any other identifier followed by
                // `{` still falls through to the error arm below. So a column
                // or binding called `type` keeps working, which matters where
                // `rdf:type` is the commonest predicate there is.
                Some(SyntaxKind::LBRACE) if p.current_text() == Some("type") => {
                    parse_type_def(p);
                }
                // `prefix ex: …` — the retired vocabulary declaration, and there
                // is no production for it. Recognised by SHAPE, not by the word:
                // `prefix` is an ordinary identifier again, so `prefix := …`
                // and `prefix : Shape from …` are matched by the two arms above
                // and never reach here. Only the three-token opening the dead
                // form had is refused, and the refusal takes the whole line
                // because the whole line is the statement.
                Some(SyntaxKind::IDENT)
                    if p.current_text() == Some("prefix")
                        && p.peek_kind(2) == Some(SyntaxKind::SHAPE_SEP) =>
                {
                    p.retire(retired::PREFIX_DECL, RetiredRun::Line);
                }
                // Always make progress on a token we don't know what to do
                // with at the program level — `bump_as_error` emits a single
                // ERROR token and advances, making the outer loop monotone
                // in `p.pos`. We CANNOT call `recover_to(p, TOP_LEVEL_ANCHORS)`
                // here because `IDENT` is itself in `TOP_LEVEL_ANCHORS` and
                // `recover_to` would no-op while the outer `loop` re-enters
                // this arm forever. That was the parser-hang on a bare `#`: the
                // bytes logos dropped silently left IDENT as the next token.
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
// PolicyDef: `policy := "people.jsonld"`
//   PolicyDef := 'policy' DEFINE STRING
//
// The ODRL document the release is verified against, and the second — and
// last — external document a program names. `fossil run` refuses to seal a
// corpus that misses the bound it declares.
//
// THREE TOKENS, NO EXPRESSION SLOT, and that is the production rather than a
// simplification of it. Every other external document arrives through the `io.`
// registry, and `io` is the TYPE-PROVIDER registry (`/docs/design/type-providers`):
// a row there runs at COMPILE time and its output is types. The compiler never
// opens a policy document — the WRITER does, after the check and immediately
// before it seals — so an `io.policy` row would join a registry whose defining
// property it lacks, and `fossil providers`, whose JSON populates a connector
// UI, would advertise it as a data source. The reference goes through
// `fossil-locator` like every other written reference, and that rule reads a
// string.
//
// The caller has verified all three tokens (rule 8 needs two of lookahead), so
// this bumps rather than expects — the opposite of `parse_type_def`, which has
// two callers and therefore may assume nothing. If a second caller ever
// appears, the three `bump`s become `expect_or_recover`s the same way.
// ───────────────────────────────────────────────────────────────────────
fn parse_policy_def(p: &mut Parser) {
    p.start(SyntaxKind::POLICY_DEF);
    p.bump(); // IDENT `policy`  (text-checked by parse_program)
    p.skip_trivia();
    p.bump(); // DEFINE  (`:=`)
    p.skip_trivia();
    p.bump(); // STRING  — the document reference
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
// TypeDef (type binding): `type { Person, City } := io.shex("s.shex")`
//   TypeDef := RenameAttr* 'type' LBRACE IDENT (COMMA IDENT)* RBRACE
//              DEFINE Expression
//
// The same destructuring as `MultiSourceDef`, in type position — one catalogue
// (`io.*`) and ONE binder. The brace list is byte-for-byte the loop above; the
// differences are the leading contextual `type` and the `RenameAttr*` prefix.
//
// `:=` and not `=` (grammar.bnf, DEFINE): `:=` BINDS A NAME and `=` ASSIGNS A
// VALUE, and what is being bound is read off the left-hand side, not off the
// glyph. Every `=` in the grammar is an assignment — a body property,
// `@subject`, a named argument.
//
// TWO ENTRY POINTS, ONE FUNCTION. `parse_program` reaches this on `AT_ATTR` and
// on the contextual `type` alike, and neither is allowed to assume the other's
// lookahead — so the `type` and the `{` are CHECKED here rather than bumped on
// the caller's word.
// ───────────────────────────────────────────────────────────────────────
fn parse_type_def(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_DEF);
    // `RenameAttr*` — zero or more, each on its own line above the binding.
    // The grammar's `*` is the permissive reading, and it is the DECIDED one:
    // open question 1 was settled on 2026-08-12 (grammar.bnf, § OPEN) in favour
    // of `RenameAttr*` with `(COMMA Rename)+`, on the argument that the
    // alternative is a LIMIT and a limit needs a reason. Refusing a second one
    // would be a ruling made by a parser, which is the inversion
    // `grammar.bnf`'s own header forbids.
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::AT_ATTR) {
            break;
        }
        parse_rename_attr(p);
    }
    p.skip_trivia();
    // The contextual `type`. Checked by TEXT, because it is an ordinary
    // identifier — that is the whole of disambiguation rule 5, and it is what
    // keeps `type` usable as a column name in a language whose commonest
    // predicate is `rdf:type`.
    if p.current() == Some(SyntaxKind::IDENT) && p.current_text() == Some("type") {
        p.bump();
    } else {
        malformed(
            p,
            "`@rename` renames a predicate of ONE type binding, so a `type { … } := …` has to \
             follow it. It goes in the program and not in the `.shex` because the vocabulary may \
             not be yours.",
        );
        p.finish();
        return;
    }
    p.skip_trivia();
    recover::expect_or_recover(p, SyntaxKind::LBRACE, TOP_LEVEL_ANCHORS);
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

/// Push a [`ParseDiagnostic::Malformed`] at the current token.
///
/// Zero-width at the token's start, which is what `Parser::expect` does for the
/// same reason: the offending token is not consumed, so underlining it would
/// claim a form this parser has not decided to take.
fn malformed(p: &mut Parser, message: &str) {
    let at = p.current_token_span_start();
    let at = u32::try_from(at).unwrap_or(u32::MAX);
    p.push_diagnostic(crate::parser::diag::ParseDiagnostic::Malformed {
        message: message.to_string(),
        span: fossil_base::Span::new(at, at),
    });
}

// ───────────────────────────────────────────────────────────────────────
// RenameAttr := AT_ATTR LPAREN IDENT (COMMA Rename)+ RPAREN
//                                                     (grammar.bnf, RenameAttr)
//
//   @rename(Person, "http://xmlns.com/foaf/0.1/name" as foaf_name)
//   type { Person } := io.shex("mixed.shex")
//
// The repair for two predicates whose last IRI segments coincide, in the shape
// of Prisma's `@map`: all constants, above the declaration, in the PROGRAM
// rather than in the `.shex` because the vocabulary may not be yours.
//
// Two predicates that collide make BOTH unwritable, and fossil never picks
// between them — the evidence is FSharp.Data's own (PLDI 2016 §6.5): its
// numeric-suffix collision scheme and its singulariser renamed members across a
// minor version and broke a program in production. So the repair is the
// author's, and it is what `check.rs`'s collision diagnostic has been telling
// people to write since before it parsed.
//
// The `IDENT` is one of the names the binding BELOW introduces; that it is one
// of them is not checked here, because the parser cannot see whether a name is
// bound. `fossil_hir::lower` checks it, against the same node, with a real span.
// ───────────────────────────────────────────────────────────────────────
fn parse_rename_attr(p: &mut Parser) {
    p.start(SyntaxKind::RENAME_ATTR);
    // The token is an `AT_ATTR`; the NAME is what decides which of the two
    // attributes this is (disambiguation rule 6). Checking it here rather than
    // in the HIR is deliberate: `@subject` in this position is not an unknown
    // name, it is a known one in the wrong place, and only the parser knows
    // which place this is.
    if p.current_text() != Some("@rename") {
        let found = p.current_text().unwrap_or("@").to_string();
        malformed(
            p,
            &format!(
                "`{found}` is not what goes above a `type` binding: the only attribute in this \
                 position is `@rename`. `@subject` is the first line of a MAPPING BODY — it \
                 assigns the identity of the rows a mapping writes, and above a type binding \
                 there are no rows yet."
            ),
        );
    }
    p.bump(); // AT_ATTR
    recover::expect_or_recover(p, SyntaxKind::LPAREN, RENAME_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::IDENT, RENAME_ANCHORS);
    // `(COMMA Rename)+` — at least one. A `@rename(Person)` renames nothing,
    // and the `+` in the grammar says so; an empty attribute would be a line
    // that looks like a repair and is not one.
    let mut renames = 0usize;
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::COMMA) {
            break;
        }
        p.bump(); // COMMA
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::RPAREN) {
            break; // trailing comma
        }
        parse_rename(p);
        renames += 1;
    }
    if renames == 0 {
        malformed(
            p,
            "`@rename` needs at least one `\"<predicate IRI>\" as <name>`: it names the type \
             whose predicate is being renamed, and then what to call it.",
        );
    }
    recover::expect_or_recover(p, SyntaxKind::RPAREN, TOP_LEVEL_ANCHORS);
    p.finish();
}

/// Anchors inside a `RenameAttr` — the two tokens that mean «the next part of
/// this attribute» and the one that closes it. `TOP_LEVEL_ANCHORS` would let a
/// recovery run out of the parentheses and eat the `type` line below.
const RENAME_ANCHORS: &[SyntaxKind] = &[
    SyntaxKind::COMMA,
    SyntaxKind::STRING,
    SyntaxKind::RPAREN,
    SyntaxKind::IDENT,
];

// ───────────────────────────────────────────────────────────────────────
// Rename := STRING 'as' IDENT                            (grammar.bnf, Rename)
//
// The first of the two places `as` survives, and one of the two that keep it
// CONTEXTUAL rather than reserved — disambiguation rule 7
// (grammar.bnf, § DISAMBIGUATION RULES). One token of
// lookahead settles it: after a STRING operand comes a bare IDENT, and no
// expression can continue that way — there is no juxtaposition in this
// language, so `"…" as x` has exactly one reading and `as` stays an ordinary
// identifier everywhere else.
//
// The predicate is a full IRI in a STRING and never a bare name: the whole
// point of the attribute is that the two colliding predicates differ only
// BEFORE their last segment, so the last segment cannot identify either.
// ───────────────────────────────────────────────────────────────────────
fn parse_rename(p: &mut Parser) {
    p.start(SyntaxKind::RENAME);
    recover::expect_or_recover(p, SyntaxKind::STRING, RENAME_ANCHORS);
    p.skip_trivia();
    // `as` — an IDENT whose text is `as`, checked exactly as the contextual
    // `type` is. It is NOT a keyword: the lexer hands it back as an ordinary
    // identifier and a column called `as` still parses everywhere else.
    if p.current() == Some(SyntaxKind::IDENT) && p.current_text() == Some("as") {
        p.bump();
    } else {
        malformed(
            p,
            "a rename is `\"<predicate IRI>\" as <name>` — the `as` is what separates the \
             predicate from the name you want to write instead.",
        );
    }
    recover::expect_or_recover(p, SyntaxKind::IDENT, RENAME_ANCHORS);
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
    // A retired absolute IRI in shape position takes the rest of the LINE with
    // it — `from` and the source expression included, because `//` turned them
    // into a comment before the parser ever saw them. Asking for a `from` that
    // the lexer ate would report a second time about a line already refused,
    // and one line of source has to produce one complaint.
    if parse_shape_expr(p) == ShapeOutcome::LineConsumed {
        p.finish();
        return;
    }
    p.skip_trivia();
    // `from` is required by the grammar but absent in some recovery
    // fixtures (e.g. fixture 16). Emit an ExpectedToken diagnostic and
    // recover toward INDENT or the next top-level anchor.
    recover::expect_or_recover(p, SyntaxKind::KW_FROM, &[SyntaxKind::INDENT]);
    // `from` takes a full Expression, and that is where the verbs live:
    // `from Adults`, `from User.where(User.age >= 18)`,
    // `from Purchase.join(User, on = Purchase.user_id == User.id)` — one
    // production for all three. A bare `from users` is the degenerate case,
    // because a bare IDENT is a valid primary expression.
    p.skip_trivia();
    if p.current() != Some(SyntaxKind::INDENT) && p.current().is_some() {
        p.parse_expr();
    }
    p.finish();
}

// ShapeExpr: IDENT   (grammar.bnf, ShapeExpr)
//
// A NAME — one of those a `type { … } := …` binding introduced — and not an IRI
// expression any more: a shape is named by the name the program bound to it, so
// `Users : Person from Adults`, never `Users : ex:Person from Adults`.
//
// One shape, and the node stays for it. The `(SHAPE_AND IRIExpr)*` tail parsed
// an intersection the HIR then reduced to its first element, silently — so
// `User : ex:Person & ex:Employee` checked against `ex:Person` alone and nobody
// was told. The `&` is not a token; the node is where the one shape lives.
//
// There is no `in g` either: the named-graph clause parsed into an IN_CLAUSE
// nothing read, and GraphAr is vertex and edge tables, not quads.

/// Whether [`parse_shape_expr`] left anything on the line for its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeOutcome {
    /// The header continues: `from` and the source expression are still there.
    HeaderContinues,
    /// A retired spelling took the rest of the line. Nothing is left to parse
    /// and nothing more should be reported about it.
    LineConsumed,
}

fn parse_shape_expr(p: &mut Parser) -> ShapeOutcome {
    p.skip_trivia();
    p.start(SyntaxKind::SHAPE_EXPR);
    // `ex:Person` — the CURIE, CONSUMED and named rather than recovered past.
    // The three tokens are taken as one form so the span underlines the whole
    // of what is being refused, and no contiguity check is needed to find them:
    // a `:` in shape position has exactly one other reading, and it is the
    // header's own, already consumed by the caller (disambiguation rule 3;
    // grammar.bnf, § DISAMBIGUATION RULES).
    if p.current() == Some(SyntaxKind::IDENT)
        && p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
        && p.peek_kind(2) == Some(SyntaxKind::IDENT)
    {
        p.retire(retired::CURIE, RetiredRun::Count(3));
        p.finish();
        // Three tokens, not the line: `from Adults` after a CURIE shape is
        // still there and still means what it says.
        return ShapeOutcome::HeaderContinues;
    }
    // `<https://example.org/Person>` — the absolute IRI, which no longer lexes
    // as one token. `<` is the comparison operator, so what arrives here is the
    // operator and its operands, and then a COMMENT that runs to the line break
    // because `//` opens one. The whole line goes.
    if p.current() == Some(SyntaxKind::LT) {
        p.retire(retired::ABSOLUTE_IRI, RetiredRun::Line);
        p.finish();
        return ShapeOutcome::LineConsumed;
    }
    recover::expect_or_recover(
        p,
        SyntaxKind::IDENT,
        &[
            SyntaxKind::KW_FROM,
            SyntaxKind::ASSIGN,
            SyntaxKind::COMMA,
            SyntaxKind::RBRACE,
            SyntaxKind::INDENT,
            SyntaxKind::DEDENT,
        ],
    );
    p.finish();
    ShapeOutcome::HeaderContinues
}

// MappingBody: Property+
fn parse_mapping_body(p: &mut Parser) {
    p.start(SyntaxKind::MAPPING_BODY);
    loop {
        p.skip_trivia();
        match p.current() {
            None | Some(SyntaxKind::DEDENT) => break,
            Some(SyntaxKind::AT_ATTR) => parse_subject_assign(p),
            // `LT` is dispatched here so that `parse_property_lhs` can CONSUME
            // the `<http://…/name>` key and name the bare key to write instead.
            // Recovering past it would turn a spelling the parser recognises
            // into `unexpected token`.
            Some(SyntaxKind::IDENT | SyntaxKind::LT) => parse_property(p),
            _ => {
                // Malformed LHS — recover to either a fresh property start
                // (the next IDENT) or the body's closing DEDENT.
                recover::recover_to(p, MAPPING_BODY_ANCHORS);
                // Guard against the no-progress case: if recover_to left us
                // on the same token (already an anchor) but it is NOT a
                // valid property starter, force a single-token bump under an
                // ERROR node so the loop terminates.
                if !matches!(
                    p.current(),
                    None | Some(SyntaxKind::DEDENT | SyntaxKind::IDENT)
                ) {
                    p.bump_as_error();
                }
            }
        }
    }
    p.finish();
}

/// `SubjectAssign := AT_ATTR ASSIGN Expression` — the mapping's identity.
///
/// `@subject = "https://shop.example/user/{User.email}"`, an ASSIGNMENT whose
/// right-hand side is read by the ordinary expression parser.
/// The call shape `@subject(iri = …)` that `a0d9bfa` built lived two hours: it
/// had to be argued each time to be «syntax wearing a sigil» in order to
/// survive its own attribute law, and no reviewed language admits a per-row
/// expression in an attribute argument. The assignment needs no such argument,
/// and the `iri` keyword — which named an argument no production takes — went
/// with it.
///
/// Three obligations the grammar states and this function does not enforce:
/// the identity is REQUIRED, there is EXACTLY ONE, and it is the FIRST line of
/// the body. They are not parse errors but facts about a mapping, and
/// `fossil_hir::body::body` — which has the ordered list and the mapping's own
/// name to put in the message — is where they are checked.
///
/// It goes inside the body rather than above the header because the row binder
/// is introduced BY the header: above it, `User` does not exist yet.
///
/// # Why this still builds a `PROPERTY` node
///
/// `SubjectAssign` and `Property` are two productions and one CST kind, and
/// that is deliberate. `fossil_hir::spans`, `fossil_hir::body::body` and
/// `fossil_ide::hover` each number a body's expressions by walking its
/// `PROPERTY` children; a separate kind here would have to be taken out of all
/// three in lockstep, and the `ExprId` misalignment those three already carry
/// would go from one shape to two. The identity is told apart by its
/// `PROPERTY_LHS` holding an `AT_ATTR`, which is what `lower_property` reads.
fn parse_subject_assign(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY);
    p.start(SyntaxKind::PROPERTY_LHS);
    p.bump(); // AT_ATTR
    p.finish();
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, MAPPING_BODY_ANCHORS);
    p.parse_expr();
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
    p.skip_trivia();
    // `<http://xmlns.com/foaf/0.1/name> = User.name` — the retired absolute-IRI
    // key. The `//` in its scheme opened a comment that swallowed the `=` and
    // the value, so the whole line is one refused form: asking for the `=` here
    // would report twice about one line AND then take the NEXT property as this
    // one's value, which is how a single dead key used to cost two.
    if p.current() == Some(SyntaxKind::LT) {
        p.start(SyntaxKind::PROPERTY_LHS);
        p.retire(retired::ABSOLUTE_IRI, RetiredRun::Line);
        p.finish();
        p.finish();
        return;
    }
    parse_property_lhs(p);
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, MAPPING_BODY_ANCHORS);
    p.parse_expr();
    p.finish();
}

// PropertyLhs: IDENT   (grammar.bnf, PropertyLhs)
//
// A bare name. `name = User.name`, never `ex:name = …`, never
// `<http://…/name> = …` and never `iri`. The name is the last segment of the
// predicate IRI the shape declares — which is what `fossil-mir` already
// computed internally, promoted to being what the author writes.
//
// The CURIE is CONSUMED here rather than recovered past, and that is the whole
// reason this is not one `expect_or_recover`. A recovery leaves the tokens
// outside `PROPERTY_LHS` and the author gets `unexpected token` — while the
// parser knew exactly what they wrote and exactly what to write instead.
//
// The absolute IRI is refused one level up, in [`parse_property`], because it
// takes the property's `=` and value with it. `iri` needs no arm at all: it is
// an ordinary identifier now, so `iri = …` is a property called `iri`, which is
// what a language whose corpus is RDF wants it to be.
fn parse_property_lhs(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY_LHS);
    p.skip_trivia();
    match p.current() {
        // `ex:name = …`. Three tokens, one form, one span. No contiguity check:
        // a ternary cannot start in key position, so `IDENT SHAPE_SEP IDENT`
        // here has exactly one reading and it is the dead one.
        Some(SyntaxKind::IDENT)
            if p.peek_kind(1) == Some(SyntaxKind::SHAPE_SEP)
                && p.peek_kind(2) == Some(SyntaxKind::IDENT) =>
        {
            p.retire(retired::CURIE, RetiredRun::Count(3));
        }
        _ => recover::expect_or_recover(p, SyntaxKind::IDENT, MAPPING_BODY_ANCHORS),
    }
    p.finish();
}

// =====================================================================
// Disambiguation-rule unit tests (Task 2a per Blocker 1)
// =====================================================================
//
// These tests pin the forks the parser actually takes, independently of the
// fixture snapshots: structural assertions on CST kinds catch the silent-bug
// class where `UPDATE_EXPECT=1` bakes the wrong shape into the baseline.
//
// They are NOT proof that the parser implements `grammar.bnf` — nothing
// mechanical checks that file against this one, and its own header says so. The
// three rules the grammar still numbers are here, and so are the five retired
// spellings, because a refusal is as much a fork as an acceptance and the one
// thing this parser must not do with a dead form is take it quietly.

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

    /// The diagnostics one parse produces, rendered.
    fn messages(src: &str) -> Vec<String> {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        super::parse_program(&mut p);
        p.diagnostics
            .iter()
            .cloned()
            .map(|d| d.to_diagnostic().message)
            .collect()
    }

    /// The `(start, end)` byte span of the one diagnostic whose message
    /// contains `needle`. Panics unless exactly one does — a retired spelling
    /// that reports twice is as wrong as one that reports never.
    fn span_of(src: &str, needle: &str) -> (u32, u32) {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        super::parse_program(&mut p);
        let hits: Vec<_> = p
            .diagnostics
            .iter()
            .cloned()
            .map(super::super::diag::ParseDiagnostic::to_diagnostic)
            .filter(|d| d.message.contains(needle))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one diagnostic containing {needle:?} for {src:?}",
        );
        (hits[0].span.start, hits[0].span.end)
    }

    /// The source text a span selects. A refusal whose span points at a
    /// plausible wrong place is worse than one that points nowhere, so every
    /// retired-spelling test below asserts on this rather than on the offsets.
    fn underlined(src: &str, span: (u32, u32)) -> &str {
        &src[span.0 as usize..span.1 as usize]
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
    // In a property a `{` opens nothing at all, in EITHER position. Worth
    // pinning: a brace with exactly zero readings is what let the rule that
    // told a record literal from an annotation block be deleted.

    #[test]
    fn brace_after_assign_is_an_error() {
        let src = "Users : Person from User\n    rec = { name = User.n }\n";
        let root = parse_str(src);
        assert!(
            descendant_kind_exists(&root, SyntaxKind::ERROR),
            "`= {{ … }}` must be an error: nothing opens a brace in value position",
        );
    }

    #[test]
    fn brace_after_an_expression_is_an_error() {
        let src = "Users : Person from User\n    name = User.name { lang = \"en\" }\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        // The property itself still parses — `User.name` is its whole value.
        assert!(
            descendant_kind_exists(&property, SyntaxKind::POSTFIX_EXPR),
            "expected the property's value `User.name` to parse as a POSTFIX_EXPR",
        );
        assert!(
            descendant_kind_exists(&root, SyntaxKind::ERROR),
            "`User.name {{ … }}` must be an error: the annotation block is gone",
        );
    }

    // ── RULE 2 is gone, and `.` has ONE reading ───────────────────────
    //
    // The rule asked whether an expression preceded the dot: with one it was
    // postfix member access, without one it was a `FieldRef` — a column of an
    // anonymous current row. The row has a name — every reference is
    // qualified, `User.name` — so a leading `.` starts nothing. What replaces the fork is a refusal that says
    // what to write, and a member access that is now the only thing a `.` does.

    #[test]
    fn a_leading_dot_is_refused_and_names_the_qualified_form() {
        let src = "Users : Person from User\n    x = .name\n";
        assert!(
            messages(src).iter().any(|m| m.contains("qualified")),
            "a leading `.` must be refused by name, got {:?}",
            messages(src),
        );
        // And the span covers the reference, not the byte that opens it.
        assert_eq!(underlined(src, span_of(src, "qualified")), ".name");
    }

    #[test]
    fn rule2_dot_after_ident_is_postfix_method_access() {
        let src = "Users : Person from User\n    x = obj.method()\n";
        let root = parse_str(src);
        let property = find_first_kind(&root, SyntaxKind::PROPERTY).expect("expected a PROPERTY");
        assert!(
            descendant_kind_exists(&property, SyntaxKind::POSTFIX_EXPR),
            "expected POSTFIX_EXPR for `obj.method()`",
        );
    }

    // ── The five retired spellings, each refused by name ───────────────
    //
    // A rejection that is mute is the worst result available here: this tree
    // has a measured case of the opposite — `lower_property` ending in a bare
    // `return None` that `body::body` skipped without a word, so a program
    // half-written in the new spelling compiled and lost its properties. Each
    // test below asserts BOTH that the message names the replacement AND that
    // the span underlines the whole retired form.

    #[test]
    fn the_vocabulary_declaration_is_refused_over_its_whole_line() {
        let src = "prefix ex: <https://example.org/>\n";
        assert_eq!(
            underlined(src, span_of(src, "no vocabulary")),
            "prefix ex: <https://example.org/>",
        );
    }

    #[test]
    fn a_curie_in_a_shape_is_refused_over_all_three_tokens() {
        let src = "Users : ex:Person from User\n    name = User.name\n";
        assert_eq!(underlined(src, span_of(src, "bare")), "ex:Person");
    }

    #[test]
    fn a_curie_in_a_property_key_is_refused_over_all_three_tokens() {
        let src = "Users : Person from User\n    ex:name = User.name\n";
        assert_eq!(underlined(src, span_of(src, "bare")), "ex:name");
    }

    // ── The absolute IRI takes its whole line, and here is why ────────
    //
    // MEASURED, and it is the one consequence of dropping `ABS_IRI` that
    // `grammar.bnf` does not mention: `//` in an IRI's scheme is the
    // COMMENT opener, and with no `AbsIri` token nothing claims those bytes
    // first. So `<http://xmlns.com/foaf/0.1/name> = User.name` lexes as
    // `< http :` and then ONE COMMENT to the line break — the `>`, the `=` and
    // the value are all inside it.
    //
    // Two things follow, and both are in these tests. The refusal spans the
    // whole line, because a span that stopped at the `:` would underline
    // `<http:` and point at a plausible wrong place. And there is nothing after
    // it to save: a program with an absolute IRI in it has already lost the
    // rest of that line to the comment lexer, whatever this parser does.

    #[test]
    fn an_absolute_iri_property_key_is_refused_over_its_whole_line() {
        let src = "Users : Person from User\n    <http://xmlns.com/foaf/0.1/name> = User.name\n";
        assert_eq!(
            underlined(src, span_of(src, "absolute IRI")),
            "<http://xmlns.com/foaf/0.1/name> = User.name",
        );
    }

    #[test]
    fn an_absolute_iri_shape_is_refused_over_its_whole_line() {
        let src = "Users : <https://example.org/Person> from User\n    name = User.name\n";
        assert_eq!(
            underlined(src, span_of(src, "absolute IRI")),
            "<https://example.org/Person> from User",
        );
    }

    /// And a `<…>` with no `//` in it — the only kind whose `>` survives —
    /// still takes the line. One rule, not two: the form is dead either way,
    /// and a second span rule for the rare shape would be a rule about the
    /// IRI's scheme.
    #[test]
    fn an_absolute_iri_with_no_scheme_slashes_also_takes_its_line() {
        let src = "Users : Person from User\n    <name> = User.name\n";
        assert_eq!(
            underlined(src, span_of(src, "absolute IRI")),
            "<name> = User.name",
        );
    }

    #[test]
    fn a_backtick_is_refused_and_names_the_quoted_string() {
        // The backtick never reaches a production — it is a byte logos rejects
        // — so `bump_as_error` is where it is named. Without that arm the file
        // would report `unexpected character`, which is true and useless: the
        // author wrote a string, in the spelling that lost.
        let src = "Users : Person from User\n    @subject = `u/${User.id}`\n";
        let msgs = messages(src);
        assert!(
            msgs.iter().any(|m| m.contains("backtick opens nothing")),
            "got {msgs:?}",
        );
        assert_eq!(underlined(src, span_of(src, "backtick opens nothing")), "`");
    }

    /// An unterminated `<…>` must not eat the rest of the FILE. The run stops
    /// at the line break, so the mapping body under it still parses.
    #[test]
    fn an_unterminated_absolute_iri_stops_at_the_line_break() {
        let src = "Users : <https://example.org/Person from User\n    name = User.name\n";
        assert_eq!(
            underlined(src, span_of(src, "absolute IRI")),
            "<https://example.org/Person from User",
        );
        // The body below it survives — one line lost, not the file.
        let root = parse_str(src);
        assert!(
            find_first_kind(&root, SyntaxKind::PROPERTY).is_some(),
            "the next line must still parse as a property",
        );
    }

    // ── RULE 3 — `:` in a MappingHeader vs `:` in a ternary ───────────

    #[test]
    fn rule3_colon_in_mapping_header_is_shape_sep() {
        let src = "Users : Person from User\n    @subject = \"u/{User.id}\"\n";
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

    /// The contiguity check is GONE (grammar.bnf, § DISAMBIGUATION RULES, rule 3)
    /// and this is what its
    /// absence has to be safe against: `a : b` with no spaces around the colon
    /// used to be the only thing telling a ternary from a `PrefixedName`, and
    /// it is now an ordinary ternary because there is no prefixed name to tell
    /// it from.
    #[test]
    fn rule3_a_tight_ternary_colon_is_still_a_ternary() {
        let src = "Users : Person from User\n    x = cond ? a:b\n";
        let root = parse_str(src);
        assert!(
            find_first_kind(&root, SyntaxKind::TERNARY_EXPR).is_some(),
            "`cond ? a:b` must be a ternary: nothing else claims the colon",
        );
        assert!(
            messages(src).is_empty(),
            "and it must parse clean, got {:?}",
            messages(src),
        );
    }

    #[test]
    fn rule3_colon_in_ternary_is_t_colon() {
        let src = "Users : Person from User\n    x = cond ? a : b\n";
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
    // `<<` claims nothing since the triple term went, so the only reading left
    // to pin is that `<` is a comparison operator.

    #[test]
    fn rule4_single_lt_is_comparison_operator() {
        let src = "Users : Person from User\n    x = a < b\n";
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

    // ── RULE 6 — `@rename` vs `@subject`, by POSITION ─────────────────

    /// The `@rename` repair for two colliding predicates, spelled out. It is
    /// the one thing the collision
    /// diagnostic in `fossil_hir::check` has been telling authors to write, and
    /// until this production existed that recommendation did not parse.
    #[test]
    fn rule6_rename_above_a_type_binding_parses() {
        let src = "@rename(Person, \"http://xmlns.com/foaf/0.1/name\" as foaf_name)\n\
                   type { Person } := io.shex(\"mixed.shex\")\n";
        let root = parse_str(src);
        let type_def = find_first_kind(&root, SyntaxKind::TYPE_DEF).expect("a TYPE_DEF");
        assert!(
            descendant_kind_exists(&type_def, SyntaxKind::RENAME_ATTR),
            "`RenameAttr*` is part of `TypeDef`, so the attribute is a CHILD of \
             the binding and not a sibling — that is what scopes a rename to one \
             binding rather than to the file",
        );
        assert!(
            descendant_kind_exists(&type_def, SyntaxKind::RENAME),
            "and each `\"…\" as name` is its own node",
        );
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
    }

    /// `RenameAttr*` — more than one, which is how open question 1 was decided
    /// on 2026-08-12 (grammar.bnf, § OPEN) and is what the `*` says.
    #[test]
    fn rule6_two_renames_and_two_attributes_both_parse() {
        let src = "@rename(Person, \"http://a/name\" as a_name, \"http://b/name\" as b_name)\n\
                   @rename(City, \"http://a/label\" as a_label)\n\
                   type { Person, City } := io.shex(\"m.shex\")\n";
        let root = parse_str(src);
        let type_def = find_first_kind(&root, SyntaxKind::TYPE_DEF).expect("a TYPE_DEF");
        let attrs = type_def
            .children()
            .filter(|c| c.kind() == SyntaxKind::RENAME_ATTR)
            .count();
        let renames = type_def
            .children()
            .filter(|c| c.kind() == SyntaxKind::RENAME_ATTR)
            .flat_map(|a| a.children())
            .filter(|c| c.kind() == SyntaxKind::RENAME)
            .count();
        assert_eq!((attrs, renames), (2, 3));
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
    }

    /// `@subject` at top level is a KNOWN name in the WRONG place, and the
    /// message says which place it belongs in. That is the whole of rule 6:
    /// one token, two names, position decides.
    #[test]
    fn rule6_subject_at_top_level_names_the_position_it_belongs_in() {
        let src = "@subject = \"x\"\ntype { Person } := io.shex(\"m.shex\")\n";
        let msgs = messages(src);
        assert!(
            msgs.iter()
                .any(|m| m.contains("@rename") && m.contains("MAPPING BODY")),
            "expected a message naming both positions, got {msgs:?}",
        );
    }

    /// And a `@rename` with no binding under it is refused rather than
    /// swallowed — a rename renames a predicate OF something.
    #[test]
    fn rule6_a_rename_with_no_type_binding_under_it_is_refused() {
        let src = "@rename(Person, \"http://a/name\" as a_name)\nUser := io.csv(\"u.csv\")\n";
        let msgs = messages(src);
        assert!(
            msgs.iter().any(|m| m.contains("has to follow it")),
            "expected the missing-binding message, got {msgs:?}",
        );
    }

    // ── RULE 7 — `as` is CONTEXTUAL ───────────────────────────────────

    /// The self-join, from `apps/docs/programs/self-join/`. The alias binds a
    /// second name for the same source so the two sides of the join can be told
    /// apart.
    #[test]
    fn rule7_the_self_join_alias_is_an_alias_arg() {
        let src = "type { Category } := io.shex(\"c.shex\")\n\
                   Node := io.csv(\"c.csv\")\n\
                   Pairs := Node.join(Node as Other, on = Node.parent == Other.id)\n";
        let root = parse_str(src);
        let alias = find_first_kind(&root, SyntaxKind::ALIAS_ARG).expect("an ALIAS_ARG");
        let idents: Vec<String> = alias
            .children_with_tokens()
            .filter_map(crate::SyntaxElement::into_token)
            .filter(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| t.text().to_string())
            .collect();
        assert_eq!(idents, vec!["Node", "as", "Other"]);
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
    }

    /// The half that makes it CONTEXTUAL rather than reserved: `as` is an
    /// ordinary identifier everywhere the two-token lookahead does not fire —
    /// a binding name, a column, a property key. A language that reserved it
    /// would refuse all three for the sake of two productions.
    #[test]
    fn rule7_as_is_an_ordinary_identifier_everywhere_else() {
        let src = "type { Person } := io.shex(\"m.shex\")\n\
                   as := io.csv(\"as.csv\")\n\
                   Users : Person from as\n    \
                   as = as.as\n";
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
        let root = parse_str(src);
        assert!(
            !descendant_kind_exists(&root, SyntaxKind::ALIAS_ARG),
            "nothing here is an alias: no argument position, no two-IDENT run",
        );
    }

    /// A named argument and an alias start the same way and the SECOND token
    /// separates them. Pinning both in one test is what keeps the fork from
    /// being read as «`IDENT` in an argument means alias».
    #[test]
    fn rule7_a_named_argument_is_not_an_alias() {
        let src = "type { Person } := io.shex(\"m.shex\")\n\
                   Node := io.csv(\"c.csv\")\n\
                   Pairs := Node.join(Node as Other, on = Node.parent)\n";
        let root = parse_str(src);
        assert!(descendant_kind_exists(&root, SyntaxKind::ALIAS_ARG));
        assert!(
            descendant_kind_exists(&root, SyntaxKind::NAMED_ARG),
            "`on = …` is still a named argument",
        );
    }

    // ── RULE 8 — `policy` at top level ────────────────────────────────

    /// The production `grammar.bnf` carried unparsed. `--policy` is a flag and
    /// a flag can be forgotten; a binding is in the file that gets reviewed.
    #[test]
    fn rule8_a_policy_binding_is_a_policy_def() {
        let src = "policy := \"people.jsonld\"\n\
                   type { Person } := io.shex(\"p.shex\")\n";
        let root = parse_str(src);
        let policy = find_first_kind(&root, SyntaxKind::POLICY_DEF).expect("a POLICY_DEF");
        assert_eq!(
            crate::ast::PolicyDef::cast(policy)
                .expect("the node is a POLICY_DEF")
                .document()
                .as_deref(),
            Some("people.jsonld"),
            "the accessor hands back what was WRITTEN, unquoted and unanchored",
        );
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
    }

    /// **The half rule 5 could not buy for `type`.** The token after `:=`
    /// decides, so the word costs nothing in the identifier space: a source
    /// called `policy` still binds, and a corpus whose commonest column is
    /// `policy` still compiles.
    #[test]
    fn rule8_policy_is_an_ordinary_identifier_everywhere_else() {
        let src = "type { Person } := io.shex(\"p.shex\")\n\
                   policy := io.csv(\"policy.csv\")\n\
                   Users : Person from policy\n    \
                   policy = policy.policy\n";
        let root = parse_str(src);
        assert!(
            !descendant_kind_exists(&root, SyntaxKind::POLICY_DEF),
            "`policy := io.csv(…)` is a SOURCE_DEF; only a STRING selects the \
             policy binding",
        );
        assert!(
            descendant_kind_exists(&root, SyntaxKind::SOURCE_DEF),
            "and it is still a source binding",
        );
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
    }

    /// An interpolated reference is NOT one. A policy reference has nothing in
    /// scope to interpolate, and the bound has to be readable off the source
    /// text without running the program — so the hole-opening token falls
    /// through to `SourceDef`, where the checker refuses it as a source.
    #[test]
    fn rule8_an_interpolated_reference_is_not_a_policy_binding() {
        let src = "policy := \"p-{users.id}.jsonld\"\n";
        let root = parse_str(src);
        assert!(!descendant_kind_exists(&root, SyntaxKind::POLICY_DEF));
    }

    /// A mapping called `policy` is still a mapping — the other fork of the
    /// same word, and the one rule 5 also has to keep open.
    #[test]
    fn rule8_a_mapping_called_policy_is_a_mapping() {
        let src = "type { Person } := io.shex(\"p.shex\")\n\
                   users := io.csv(\"u.csv\")\n\
                   policy : Person from users\n    \
                   name = users.name\n";
        let root = parse_str(src);
        assert!(descendant_kind_exists(&root, SyntaxKind::MAPPING));
        assert!(!descendant_kind_exists(&root, SyntaxKind::POLICY_DEF));
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
    }

    /// The CST stays LOSSLESS across the new node: every byte of the binding is
    /// still under it. A production that dropped its own text would be
    /// invisible to the LSP and to any formatter.
    #[test]
    fn rule8_the_policy_binding_is_lossless() {
        let src = "policy   :=   \"a/b/people.jsonld\"\n";
        let root = parse_str(src);
        let policy = find_first_kind(&root, SyntaxKind::POLICY_DEF).expect("a POLICY_DEF");
        assert_eq!(
            policy.text().to_string(),
            "policy   :=   \"a/b/people.jsonld\""
        );
    }
}
