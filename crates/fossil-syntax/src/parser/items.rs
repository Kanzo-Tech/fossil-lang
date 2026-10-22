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
//!                      | TypeDef | TypeDecl | Mapping      — as specified
//! MultiSourceDef      := LBRACE ImportMember (COMMA ImportMember)* RBRACE
//!                        DEFINE Expression                 — as specified
//! TypeDef             := 'type' LBRACE ImportMember (COMMA ImportMember)* RBRACE
//!                        DEFINE Expression                 — as specified
//! ImportMember        := IDENT ('as' IDENT)?               — as specified
//! TypeDecl            := DeclAttrs 'type' IDENT NEWLINE INDENT Field+ DEDENT
//!                                                          — as specified
//! Field               := DeclAttrs IDENT SHAPE_SEP TypeExpr NEWLINE
//!                                                          — as specified
//! DeclAttr            := IriAttr | AnnotationAttr
//!                      | AssertAttr | SeverityAttr         — as specified
//! TypeExpr            := IDENT (LT TypeExpr
//!                        (COMMA INTEGER (COMMA INTEGER)?)? GT)?
//!                                                          — as specified
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
//! Three of them live in this module — the vocabulary declaration
//! ([`parse_program`]), the CURIE in a shape or a property key
//! ([`parse_shape_expr`], [`parse_property_lhs`]) and the `<…>` absolute IRI
//! ([`parse_shape_expr`], [`parse_property`]). Each CONSUMES its form under an
//! `ERROR` node and
//! reports [`super::diag::retired`]'s message for it, because leaving the
//! tokens to `recover_to` produces `unexpected token` for something the parser
//! recognised exactly.
//!
//! # Disambiguation rules (grammar.bnf, § DISAMBIGUATION RULES)
//!
//! What this module and `super::expr` implement, numbered as the grammar
//! numbers them and exercised by `mod disambiguation` at the bottom of this
//! file. A `{` in a mapping body is one thing, an error; a `.` is only ever
//! member access, because every reference is qualified (`User.name`); `<` is
//! only ever a comparison.
//!
//! 3. `:` in a `MappingHeader` vs `:` in a ternary: one lexeme, one token
//!    (`SHAPE_SEP`), consumed by [`parse_mapping_header`] in the first case
//!    and by [`super::expr::parse_expression`]'s ternary handler in the
//!    second. The node is what tells them apart, not the token, and
//!    whitespace around the colon means nothing.
//! 5. `type` at top level: `type {` is a `TypeDef`, `type IDENT` a
//!    `TypeDecl`, `type :=` a `SourceDef` binding the name `type`, `type :` a
//!    mapping called `type`. One token of lookahead, in [`parse_program`], and
//!    the word stays a column name.
//! 6. a declaration's attributes vs `@subject`: all lex as `AT_ATTR` and POSITION decides.
//!    [`parse_iri_attr`] takes the top-level position — `TypeDecl` starts with
//!    `AT_ATTR`, and it is that token's only derivation at top level — and the
//!    start of a line in a declaration's body; [`parse_subject_assign`] takes
//!    the first line of a mapping body. Both bodies are reachable only through
//!    an `INDENT`, and the item above each says which one it is. Any other
//!    name at top level is refused by [`parse_program`] and in a declaration
//!    by [`parse_iri_attr`]; `fossil_hir` refuses one in a mapping body. So
//!    neither is valid in the other's position and no third name is valid
//!    anywhere.
//! 7. `as`, contextual in `IDENT as IDENT` twice: an import member
//!    ([`parse_import_member`]) and the self-join alias
//!    (`super::expr::parse_arg`). One token of lookahead settles both: an
//!    operand followed by a bare `IDENT`, which no expression can continue
//!    because the language has no juxtaposition. Everywhere else `as` is an
//!    ordinary identifier and lexes as one.

use fossil_graph_schema::Problem;

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
            // `@iri("…")` and `@annotation(…)` above a `type` declaration.
            // LL(1) survives them because `AT_ATTR` has exactly ONE derivation
            // here: `SubjectAssign` and a field's attributes live inside a
            // body, which is reached only through `INDENT` (disambiguation
            // rule 6). Any other name, and an attribute above an import, is
            // consumed whole and refused, so the item under it still parses as
            // what it is.
            Some(SyntaxKind::AT_ATTR) if !is_decl_attr(p.current_text()) => {
                let found = p.current_text().unwrap_or("@").to_string();
                parse_misplaced_attr(
                    p,
                    found,
                    "at the top level",
                    "the attributes at the top level are `@iri(\"…\")` and `@annotation(…)`, \
                     above a `type` declaration. `@subject = <expr>` is the first line of a \
                     MAPPING BODY — it assigns the identity of the rows a mapping writes, and \
                     outside one there are no rows.",
                );
            }
            Some(SyntaxKind::AT_ATTR) if above_an_import(p) => {
                let found = p.current_text().unwrap_or("@").to_string();
                parse_misplaced_attr(
                    p,
                    found,
                    "above a `type` import",
                    "an imported type takes its IRI and its annotations from the document that \
                     declares it. `@iri` and `@annotation` belong to a type DECLARED under them, \
                     `type <Name>` and its fields.",
                );
            }
            Some(SyntaxKind::AT_ATTR) => parse_type_decl(p),
            // `{ A, B, ... } := io.rdf(...)` — a destructuring source def, and
            // the only thing a top-level `{` opens.
            Some(SyntaxKind::LBRACE) => parse_multi_source_def(p),
            Some(SyntaxKind::IDENT) => match p.peek_kind(1) {
                // `IDENT :=` → source binding (SOURCE_DEF).
                Some(SyntaxKind::DEFINE) => parse_source_def(p),
                // `IDENT :` → start of a mapping header.
                Some(SyntaxKind::SHAPE_SEP) => parse_mapping(p),
                // `type { A, B } := …` → a type binding (TYPE_DEF). `type` is a
                // CONTEXTUAL keyword, not a reserved word: the slot `IDENT {`
                // is otherwise free at top level, and any other identifier
                // followed by `{` falls through to the error arm below. So a column
                // or binding called `type` keeps working, which matters where
                // `rdf:type` is the commonest predicate there is.
                Some(SyntaxKind::LBRACE) if p.current_text() == Some("type") => {
                    parse_type_def(p);
                }
                // `type Person` → a declaration whose `@iri` is missing. No
                // other item puts two `IDENT`s side by side, so it is parsed as
                // one and refused for the line it lacks, rather than recovered
                // past as `unexpected token`.
                Some(SyntaxKind::IDENT) if p.current_text() == Some("type") => {
                    parse_type_decl(p);
                }
                // Always make progress on a token we don't know what to do
                // with at the program level — `bump_as_error` emits a single
                // ERROR token and advances, making the outer loop monotone
                // in `p.pos`. We CANNOT call `recover_to(p, TOP_LEVEL_ANCHORS)`
                // here because `IDENT` is itself in `TOP_LEVEL_ANCHORS` and
                // `recover_to` would no-op while the outer `loop` re-enters
                // this arm forever — a parser hang on a bare `#`, whose bytes
                // the lossy lex drops, leaving IDENT as the next token.
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
// `adults := users.where(...)` derives a relation; both are a name, `:=`
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
//   MultiSourceDef := LBRACE ImportMember (COMMA ImportMember)* RBRACE
//                     DEFINE Expression
//
// Each member names a shape declared in the source's schema, bound to the
// single source on the right of `:=`. The member list is `TypeDef`'s, by the
// grammar's own word: the two must not drift apart.
// ───────────────────────────────────────────────────────────────────────
fn parse_multi_source_def(p: &mut Parser) {
    p.start(SyntaxKind::MULTI_SOURCE_DEF);
    parse_import_members(p);
    p.skip_trivia();
    recover::expect_or_recover(p, SyntaxKind::DEFINE, TOP_LEVEL_ANCHORS);
    p.parse_expr();
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// TypeDef (type import): `type { Place as City } := io.shex("places.shex")`
//   TypeDef := 'type' LBRACE ImportMember (COMMA ImportMember)* RBRACE
//              DEFINE Expression
//
// `:=` and not `=` (grammar.bnf, DEFINE): `:=` BINDS A NAME and `=` ASSIGNS A
// VALUE, and what is being bound is read off the left-hand side, not off the
// glyph. Every `=` in the grammar is an assignment — a body property,
// `@subject`, a named argument.
// ───────────────────────────────────────────────────────────────────────
fn parse_type_def(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_DEF);
    p.bump(); // the contextual `type`, checked by `parse_program`'s lookahead
    parse_import_members(p);
    p.skip_trivia();
    recover::expect_or_recover(p, SyntaxKind::DEFINE, TOP_LEVEL_ANCHORS);
    p.parse_expr();
    p.finish();
}

/// `LBRACE ImportMember (COMMA ImportMember)* RBRACE` — the member list both
/// destructurings share. A trailing comma is allowed.
fn parse_import_members(p: &mut Parser) {
    recover::expect_or_recover(p, SyntaxKind::LBRACE, TOP_LEVEL_ANCHORS);
    parse_import_member(p);
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::COMMA) {
            break;
        }
        p.bump(); // COMMA
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::RBRACE) {
            break;
        }
        parse_import_member(p);
    }
    recover::expect_or_recover(p, SyntaxKind::RBRACE, TOP_LEVEL_ANCHORS);
}

/// Anchors inside a member list: the separator and the closer. Wider sets would
/// let a recovery run out of the braces and eat the `:=` after them.
const MEMBER_ANCHORS: &[SyntaxKind] = &[SyntaxKind::COMMA, SyntaxKind::RBRACE];

// ───────────────────────────────────────────────────────────────────────
// ImportMember := IDENT ('as' IDENT)?
//
// The first IDENT is the LOCAL NAME of a shape the document declares, and the
// one after `as` is what the program calls it. `as` is checked by TEXT, as the
// contextual `type` is (disambiguation rule 7): inside a member list nothing
// else follows a name but `,` or `}`, so an IDENT there can only be `as`.
// ───────────────────────────────────────────────────────────────────────
fn parse_import_member(p: &mut Parser) {
    p.skip_trivia();
    p.start(SyntaxKind::IMPORT_MEMBER);
    recover::expect_or_recover(p, SyntaxKind::IDENT, MEMBER_ANCHORS);
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::IDENT) && p.current_text() == Some("as") {
        p.bump();
        recover::expect_or_recover(p, SyntaxKind::IDENT, MEMBER_ANCHORS);
    }
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// TypeDecl := DeclAttrs 'type' IDENT NEWLINE INDENT Field+ DEDENT
//
//   @iri("https://schema.org/Person")
//   @annotation("http://www.w3.org/2000/01/rdf-schema#label", "person"@en)
//   type Person
//       @iri("https://schema.org/name")  name: String
//
// An output type written in the program. The body is indented as a mapping's
// is, and there is no `=`: a declaration assigns nothing.
//
// Reached on `AT_ATTR` and on `type IDENT` alike. The second is a declaration
// that forgot its IRI, and is parsed whole so the refusal names it.
// ───────────────────────────────────────────────────────────────────────
fn parse_type_decl(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_DECL);
    let attrs = parse_decl_attrs(p, "above a `type` declaration", false);
    p.skip_trivia();
    let is_type = p.current() == Some(SyntaxKind::IDENT) && p.current_text() == Some("type");
    if is_type {
        let name = p.peek_text(1).unwrap_or("type").to_string();
        attrs.check(p, name);
    }
    // The contextual `type`, checked by TEXT: it is an ordinary identifier, and
    // that is what keeps it usable as a column name.
    if is_type {
        p.bump();
    } else {
        malformed(
            p,
            Problem::MisplacedAttribute {
                attribute: crate::attr::IRI.to_string(),
                place: "anywhere but above a `type` declaration".to_string(),
            },
            "`@iri` names the IRI of the type declared under it, so `type <Name>` and its \
             fields have to follow. An imported type takes its IRI from the document.",
        );
        p.finish();
        return;
    }
    recover::expect_or_recover(
        p,
        SyntaxKind::IDENT,
        &[SyntaxKind::INDENT, SyntaxKind::IDENT],
    );
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::INDENT) {
        p.bump();
        parse_fields(p);
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::DEDENT) {
            p.bump();
        } else {
            recover::recover_to(p, TOP_LEVEL_ANCHORS);
        }
    } else {
        // `Field+`: a declaration with no body declares a type with nothing
        // in it, which no mapping could write.
        recover::expect_or_recover(p, SyntaxKind::INDENT, TOP_LEVEL_ANCHORS);
    }
    p.finish();
}

/// Is `text` the name of an attribute a declaration takes — `@iri`,
/// `@annotation`, `@assert` or `@severity`?
fn is_decl_attr(text: Option<&str>) -> bool {
    matches!(
        text,
        Some(
            crate::attr::IRI
                | crate::attr::ANNOTATION
                | crate::attr::ASSERT
                | crate::attr::SEVERITY
        )
    )
}

/// Do the attributes starting here stand above a `type { … }` import? Skips
/// each `AT_ATTR` and its parenthesised arguments, then looks at what follows:
/// one token of lookahead past the attributes, as rule 6 has it.
fn above_an_import(p: &Parser) -> bool {
    let mut i = 0;
    while p.peek_kind(i) == Some(SyntaxKind::AT_ATTR) {
        i += 1;
        if p.peek_kind(i) == Some(SyntaxKind::LPAREN) {
            let mut depth = 0usize;
            loop {
                match p.peek_kind(i) {
                    None => return false,
                    Some(SyntaxKind::LPAREN) => depth += 1,
                    Some(SyntaxKind::RPAREN) => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
    }
    p.peek_text(i) == Some("type") && p.peek_kind(i + 1) == Some(SyntaxKind::LBRACE)
}

/// What a run of attributes held, for the checks a declaration owes them.
struct DeclAttrs {
    iris: usize,
    severities: usize,
    /// An attribute of another kind was written, and refused: a missing `@iri`
    /// is then not a second report about the same line.
    misplaced: bool,
}

impl DeclAttrs {
    /// `@iri` is exactly one per type and per field and `@severity` at most
    /// one per field (grammar.bnf, `DeclAttrs`); `@annotation` and `@assert`
    /// any number.
    fn check(&self, p: &mut Parser, name: String) {
        if self.severities > 1 {
            malformed(
                p,
                Problem::DuplicateAttribute {
                    attribute: crate::attr::SEVERITY.to_string(),
                    name: name.clone(),
                },
                "a field's count is enforced one way. Keep the severity meant; an assertion \
                 carries its own, `@assert(…, severity = warning)`.",
            );
        }
        match self.iris {
            0 if !self.misplaced => missing_iri(p, name),
            0 | 1 => {}
            _ => malformed(
                p,
                Problem::DuplicateAttribute {
                    attribute: crate::attr::IRI.to_string(),
                    name,
                },
                "a type and a field each have one IRI. Keep the one that names the predicate \
                 or the class meant, and put anything else said about it in an `@annotation`.",
            ),
        }
    }
}

// ───────────────────────────────────────────────────────────────────────
// DeclAttrs := DeclAttr+ (NEWLINE DeclAttr+)* NEWLINE?
// DeclAttr  := IriAttr | AnnotationAttr | AssertAttr | SeverityAttr
//
// On the line of what they describe, or stacked on the lines above it. A
// stacked attribute starts its line at the same column as the field, so the
// indent pass emits nothing for it, and a NEWLINE is trivia: the run is simply
// every `AT_ATTR` before the name.
// ───────────────────────────────────────────────────────────────────────
fn parse_decl_attrs(p: &mut Parser, place: &str, on_field: bool) -> DeclAttrs {
    let mut attrs = DeclAttrs {
        iris: 0,
        severities: 0,
        misplaced: false,
    };
    loop {
        p.skip_trivia();
        if p.current() != Some(SyntaxKind::AT_ATTR) {
            return attrs;
        }
        match p.current_text() {
            Some(crate::attr::IRI) => {
                attrs.iris += 1;
                parse_iri_attr(p);
            }
            Some(crate::attr::ANNOTATION) => parse_annotation_attr(p),
            Some(crate::attr::ASSERT) => parse_assert_attr(p),
            Some(crate::attr::SEVERITY) if on_field => {
                attrs.severities += 1;
                parse_severity_attr(p);
            }
            // A type has no count of its own to enforce: how many nodes it has
            // is not something it declares.
            Some(crate::attr::SEVERITY) => {
                attrs.misplaced = true;
                parse_misplaced_attr(
                    p,
                    crate::attr::SEVERITY.to_string(),
                    place,
                    "`@severity` says how a FIELD's count is enforced, and a type declares no \
                     count. An assertion on a type carries its own: \
                     `@assert(…, severity = warning)`.",
                );
            }
            // `@subject` here is not an unknown name, it is a known one in the
            // wrong place, and only the parser knows which place.
            found => {
                let found = found.unwrap_or("@").to_string();
                attrs.misplaced = true;
                parse_misplaced_attr(
                    p,
                    found,
                    place,
                    "the attributes in this position are `@iri(\"…\")`, `@annotation(…)`, \
                     `@assert(…)` and, before a field, `@severity(…)`. `@subject` is the first line of a MAPPING BODY — it assigns the identity of \
                     the rows a mapping writes, and a declaration writes no rows.",
                );
            }
        }
    }
}

/// Anchors inside a declaration's body: the start of the next field — its
/// `@iri`, or the name of one that forgot it — and the body's end.
const FIELD_ANCHORS: &[SyntaxKind] = &[SyntaxKind::DEDENT, SyntaxKind::AT_ATTR, SyntaxKind::IDENT];

fn parse_fields(p: &mut Parser) {
    loop {
        p.skip_trivia();
        match p.current() {
            None | Some(SyntaxKind::DEDENT) => break,
            Some(SyntaxKind::AT_ATTR | SyntaxKind::IDENT) => parse_field(p),
            _ => {
                // The no-progress guard `parse_mapping_body` carries, for the
                // same reason: an anchor that starts no field must still move.
                recover::recover_to(p, FIELD_ANCHORS);
                if !matches!(
                    p.current(),
                    None | Some(SyntaxKind::DEDENT | SyntaxKind::AT_ATTR | SyntaxKind::IDENT)
                ) {
                    p.bump_as_error();
                }
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────
// Field := DeclAttrs IDENT SHAPE_SEP TypeExpr NEWLINE
//
// A field with no `@iri` is still parsed whole, so the refusal names it and the
// next field is not lost to recovery.
// ───────────────────────────────────────────────────────────────────────
fn parse_field(p: &mut Parser) {
    p.start(SyntaxKind::FIELD);
    let attrs = parse_decl_attrs(p, "before a field of a `type` declaration", true);
    let name = p.current_text().unwrap_or_default().to_string();
    attrs.check(p, name);
    recover::expect_or_recover(p, SyntaxKind::IDENT, FIELD_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::SHAPE_SEP, FIELD_ANCHORS);
    p.skip_trivia();
    parse_type_expr(p);
    p.finish();
}

/// Anchors inside a `TypeExpr`: its closer, and the next field or the body's
/// end. Not `IDENT`, which is what a type is made of.
const TYPE_EXPR_ANCHORS: &[SyntaxKind] = &[SyntaxKind::GT, SyntaxKind::DEDENT, SyntaxKind::AT_ATTR];

// ───────────────────────────────────────────────────────────────────────
// TypeExpr := IDENT (LT TypeExpr (COMMA INTEGER (COMMA INTEGER)?)? GT)?
//
// `<` and `>` are LT and GT, and in type position nothing else can follow an
// IDENT, so they open and close and never compare. Which names take an
// argument, and which take bounds, and which are types at all, is the
// checker's: `Ref<String>` and `Option<String, 2>` parse.
// ───────────────────────────────────────────────────────────────────────
fn parse_type_expr(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_EXPR);
    recover::expect_or_recover(p, SyntaxKind::IDENT, TYPE_EXPR_ANCHORS);
    // `peek_kind` and not `skip_trivia`: the line break after a field's type
    // belongs to the body, not to this node.
    if p.peek_kind(0) == Some(SyntaxKind::LT) {
        p.skip_trivia();
        p.bump(); // LT
        p.skip_trivia();
        parse_type_expr(p);
        // The bounds: a minimum, then a maximum. A third is not a bound, and
        // is left to the `GT` that does not come.
        for _ in 0..2 {
            p.skip_trivia();
            if p.current() != Some(SyntaxKind::COMMA) {
                break;
            }
            p.bump(); // COMMA
            recover::expect_or_recover(p, SyntaxKind::INTEGER, TYPE_EXPR_ANCHORS);
        }
        recover::expect_or_recover(p, SyntaxKind::GT, TYPE_EXPR_ANCHORS);
    }
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// IriAttr := AT_ATTR LPAREN STRING RPAREN
//
// The IRI is whole, in a STRING, and constant: there is no base it is resolved
// against and no prefix it expands through. A string with a hole is not a
// `STRING` token, so it is refused here as the token it is.
// ───────────────────────────────────────────────────────────────────────
fn parse_iri_attr(p: &mut Parser) {
    p.start(SyntaxKind::IRI_ATTR);
    p.bump(); // AT_ATTR, `@iri` — `parse_decl_attrs` read its name
    recover::expect_or_recover(p, SyntaxKind::LPAREN, IRI_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::STRING, IRI_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::RPAREN, IRI_ANCHORS);
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// AnnotationAttr   := AT_ATTR LPAREN STRING COMMA AnnotationObject RPAREN
// AnnotationObject := STRING | LANG_STRING
//
// ShEx's `// predicate object`, as this language's attribute: the predicate is
// a whole IRI in a string, as `@iri`'s is, and the object a constant literal.
// Whether the predicate is a prefixed name, and whether the tag is well-formed,
// is the checker's.
// ───────────────────────────────────────────────────────────────────────
fn parse_annotation_attr(p: &mut Parser) {
    p.start(SyntaxKind::ANNOTATION_ATTR);
    p.bump(); // AT_ATTR, `@annotation`
    recover::expect_or_recover(p, SyntaxKind::LPAREN, IRI_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::STRING, IRI_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::COMMA, IRI_ANCHORS);
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::LANG_STRING) {
        p.bump();
    } else {
        recover::expect_or_recover(p, SyntaxKind::STRING, IRI_ANCHORS);
    }
    recover::expect_or_recover(p, SyntaxKind::RPAREN, IRI_ANCHORS);
    p.finish();
}

// ───────────────────────────────────────────────────────────────────────
// AssertAttr := AT_ATTR LPAREN Expression (COMMA 'severity' ASSIGN IDENT)? RPAREN
//
// The condition is an ordinary expression, under an `EXPR` node, and what it
// may name is the checker's. `severity` is contextual, as `as` is: after the
// condition's comma nothing else can stand, so it is read by text.
// ───────────────────────────────────────────────────────────────────────
fn parse_assert_attr(p: &mut Parser) {
    p.start(SyntaxKind::ASSERT_ATTR);
    p.bump(); // AT_ATTR, `@assert`
    recover::expect_or_recover(p, SyntaxKind::LPAREN, IRI_ANCHORS);
    p.parse_expr();
    p.skip_trivia();
    if p.current() == Some(SyntaxKind::COMMA) {
        p.bump();
        p.skip_trivia();
        if p.current() == Some(SyntaxKind::IDENT) && p.current_text() != Some(SEVERITY_PARAM) {
            let parameter = p.current_text().unwrap_or_default().to_string();
            malformed(
                p,
                Problem::UnknownParameter {
                    function: crate::attr::ASSERT.to_string(),
                    parameter,
                    parameters: vec![SEVERITY_PARAM.to_string()],
                },
                "an assertion takes its condition and, after it, how it is enforced: \
                 `@assert(edad >= 0, severity = warning)`.",
            );
        }
        recover::expect_or_recover(p, SyntaxKind::IDENT, IRI_ANCHORS);
        recover::expect_or_recover(p, SyntaxKind::ASSIGN, IRI_ANCHORS);
        recover::expect_or_recover(p, SyntaxKind::IDENT, IRI_ANCHORS);
    }
    recover::expect_or_recover(p, SyntaxKind::RPAREN, IRI_ANCHORS);
    p.finish();
}

/// The one named argument `@assert` takes.
const SEVERITY_PARAM: &str = "severity";

// ───────────────────────────────────────────────────────────────────────
// SeverityAttr := AT_ATTR LPAREN IDENT RPAREN
//
// Which names are severities is the checker's, as which names are types is.
// ───────────────────────────────────────────────────────────────────────
fn parse_severity_attr(p: &mut Parser) {
    p.start(SyntaxKind::SEVERITY_ATTR);
    p.bump(); // AT_ATTR, `@severity`
    recover::expect_or_recover(p, SyntaxKind::LPAREN, IRI_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::IDENT, IRI_ANCHORS);
    recover::expect_or_recover(p, SyntaxKind::RPAREN, IRI_ANCHORS);
    p.finish();
}

/// An attribute in a position that takes none of its kind, refused by name and
/// consumed whole under an `ERROR` — its `( … )` or its `= <expr>` — so that
/// recovery does not report the rest of the line a second time.
fn parse_misplaced_attr(p: &mut Parser, attribute: String, place: &str, help: &str) {
    p.start(SyntaxKind::ERROR);
    malformed(
        p,
        Problem::MisplacedAttribute {
            attribute,
            place: place.to_string(),
        },
        help,
    );
    p.bump(); // AT_ATTR
    p.skip_trivia();
    match p.current() {
        Some(SyntaxKind::ASSIGN) => {
            p.bump();
            p.parse_expr();
        }
        Some(SyntaxKind::LPAREN) => loop {
            match p.current() {
                None | Some(SyntaxKind::NEWLINE) => break,
                Some(SyntaxKind::RPAREN) => {
                    p.bump();
                    break;
                }
                _ => p.bump(),
            }
        },
        _ => {}
    }
    p.finish();
}

/// Anchors inside an `IriAttr`: its closer, and what follows one — the `type`
/// or the field's name — or the body's edges. Wider sets would let a recovery
/// run past the line.
const IRI_ANCHORS: &[SyntaxKind] = &[
    SyntaxKind::RPAREN,
    SyntaxKind::AT_ATTR,
    SyntaxKind::IDENT,
    SyntaxKind::INDENT,
    SyntaxKind::DEDENT,
];

/// Report a declaration or a field with no `@iri`, at the token where it should
/// have begun. Nothing is consumed: the declaration goes on being parsed.
fn missing_iri(p: &mut Parser, name: String) {
    malformed(
        p,
        Problem::MissingIri { name },
        "write it above a type and before each field, whole and in a string: \
         `@iri(\"https://schema.org/name\")`. There is no base it could fall back to and no \
         prefix it could expand through, so a name nobody wrote never becomes an IRI.",
    );
}

/// Push a [`ParseDiagnostic::Malformed`] carrying `problem`, with `help`, at
/// the current token.
///
/// Zero-width at the token's start, which is what `Parser::expect` does for the
/// same reason: the offending token is not consumed, so underlining it would
/// claim a form this parser has not decided to take.
fn malformed(p: &mut Parser, problem: Problem, help: &str) {
    let at = p.current_token_span_start();
    let at = u32::try_from(at).unwrap_or(u32::MAX);
    p.push_diagnostic(crate::parser::diag::ParseDiagnostic::Malformed {
        problem,
        help: Some(help.to_string()),
        span: fossil_graph_schema::Span::new(at, at),
    });
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
        // Body missing — emit recovery and continue past. `MappingBody :=
        // SubjectAssign Property+` (grammar.bnf), so a missing body is a parse
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
// expression: a shape is named by the name the program bound to it, so
// `Users : Person from Adults`, never `Users : ex:Person from Adults`.
//
// One shape, and the node is where it lives: `&` is not a token, so there is no
// intersection to reduce. There is no `in g` either: a fossil graph is
// vertex and edge types, not quads.

fn parse_shape_expr(p: &mut Parser) {
    p.skip_trivia();
    p.start(SyntaxKind::SHAPE_EXPR);
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
}

// MappingBody: (SubjectAssign | Property)* — see this module's header for the
// three obligations `fossil_hir::body::body` checks instead of this function.
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
/// right-hand side is read by the ordinary expression parser. Not a call shape,
/// `@subject(iri = …)`: no reviewed language admits a per-row expression in an
/// attribute argument, and the assignment needs no argument name.
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
// anywhere in a property is an error rather than a fork, and
// `parse_mapping_body`'s recovery arm reports it.
fn parse_property(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY);
    p.skip_trivia();
    parse_property_lhs(p);
    recover::expect_or_recover(p, SyntaxKind::ASSIGN, MAPPING_BODY_ANCHORS);
    p.parse_expr();
    p.finish();
}

// PropertyLhs: IDENT   (grammar.bnf, PropertyLhs)
//
// A bare name: the last segment of the predicate IRI the shape declares.
fn parse_property_lhs(p: &mut Parser) {
    p.start(SyntaxKind::PROPERTY_LHS);
    p.skip_trivia();
    recover::expect_or_recover(p, SyntaxKind::IDENT, MAPPING_BODY_ANCHORS);
    p.finish();
}

// =====================================================================
// Disambiguation-rule unit tests
// =====================================================================
//
// These tests pin the forks the parser actually takes, independently of the
// fixture snapshots: structural assertions on CST kinds catch the silent-bug
// class where `UPDATE_EXPECT=1` bakes the wrong shape into the baseline.
//
// They are NOT proof that the parser implements `grammar.bnf` — nothing
// mechanical checks that file against this one, and its own header says so.
// Rules 3, 6, 7 and 8 are here (rule 5's fork is fixture `31_type_def_and_
// type_as_a_binding_name`).

#[cfg(test)]
mod disambiguation {
    use crate::SyntaxKind;
    use crate::ast::{Field, ImportMember, TypeDecl, TypeExpr};
    use crate::indent::lex_with_indents;
    use crate::kind::SyntaxNode;
    use fossil_graph_schema::Problem;
    use rowan::GreenNode;

    use super::super::Parser;

    fn parse_str(src: &str) -> SyntaxNode {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        super::parse_program(&mut p);
        let green: GreenNode = p.builder.finish();
        SyntaxNode::new_root(green)
    }

    /// The problems one parse reports.
    fn messages(src: &str) -> Vec<Problem> {
        let tokens = lex_with_indents(src);
        let mut p = Parser::new(tokens);
        super::parse_program(&mut p);
        p.diagnostics
            .iter()
            .cloned()
            .map(|d| d.to_diagnostic().problem)
            .collect()
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
    // pinning: a brace with exactly zero readings needs no rule to tell its
    // readings apart.

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
            "`User.name {{ … }}` must be an error: nothing opens a brace after a value",
        );
    }

    // ── `.` has ONE reading ───────────────────────────────────────────
    //
    // The row has a name — every reference is qualified, `User.name` — so a
    // leading `.` starts nothing: it is refused with what to write, and member
    // access is the only thing a `.` does.

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

    // ── The retired spellings, each refused by name ────────────────────
    //
    // A rejection that is mute is the worst result available here: a property
    // the lowering skips without a word compiles a program that has lost it.
    // Each test below asserts BOTH that the message names the replacement AND
    // that the span underlines the whole retired form.

    // ── The absolute IRI takes its whole line, and here is why ────────
    //
    // MEASURED, and `grammar.bnf` does not mention it: `//` in an IRI's
    // scheme is the COMMENT opener, and with no IRI token nothing claims those
    // bytes first. So `<http://xmlns.com/foaf/0.1/name> = User.name` lexes as
    // `< http :` and then ONE COMMENT to the line break — the `>`, the `=` and
    // the value are all inside it.
    //
    // Two things follow, and both are in these tests. The refusal spans the
    // whole line, because a span that stopped at the `:` would underline
    // `<http:` and point at a plausible wrong place. And there is nothing after
    // it to save: a program with an absolute IRI in it has already lost the
    // rest of that line to the comment lexer, whatever this parser does.

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

    /// Whitespace around the colon means nothing
    /// (grammar.bnf, § DISAMBIGUATION RULES, rule 3): `a:b` with no spaces is an
    /// ordinary ternary, because there is no prefixed name to tell it from.
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
        // header takes, so the disambiguator is the surrounding node
        // (TERNARY_EXPR exists at all), never the token. Additionally assert T_QUESTION is one of its token children
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
    // `<<` claims nothing, so the only reading to pin is that `<` is a
    // comparison operator.

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

    // ── RULE 6 — `@iri` vs `@subject`, by POSITION ────────────────────

    /// The declaration `/docs/design/declared` opens with: two fields whose
    /// IRIs end alike, an `Option`, a `Ref`, and a `Seq` — each field one
    /// `FIELD` with its own `IRI_ATTR`, and the type's `IRI_ATTR` a child of
    /// the declaration rather than a sibling of it.
    #[test]
    fn rule6_iri_above_a_type_and_before_each_field_parses() {
        let src = "@iri(\"https://schema.org/Person\")\n\
                   type Person\n    \
                   @iri(\"https://schema.org/name\")          name: String\n    \
                   @iri(\"https://shop.example/vocab/name\")  handle: String\n    \
                   @iri(\"https://schema.org/birthDate\")     born: Option<Date>\n    \
                   @iri(\"https://schema.org/homeLocation\")  livesIn: Ref<City>\n    \
                   @iri(\"https://schema.org/knowsAbout\")    topics: Seq<String>\n";
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
        let root = parse_str(src);
        let decl = find_first_kind(&root, SyntaxKind::TYPE_DECL)
            .and_then(TypeDecl::cast)
            .expect("a TYPE_DECL");
        assert_eq!(decl.name().as_deref(), Some("Person"));
        assert_eq!(
            decl.iri_attr().and_then(|a| a.iri()).as_deref(),
            Some("https://schema.org/Person"),
            "the type's `@iri` belongs to the declaration it names",
        );
        let fields: Vec<(String, String)> = decl
            .fields()
            .map(|f| {
                (
                    f.name().unwrap_or_default().to_string(),
                    f.iri_attr()
                        .and_then(|a| a.iri())
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            [
                ("name", "https://schema.org/name"),
                ("handle", "https://shop.example/vocab/name"),
                ("born", "https://schema.org/birthDate"),
                ("livesIn", "https://schema.org/homeLocation"),
                ("topics", "https://schema.org/knowsAbout"),
            ]
            .map(|(n, i)| (n.to_string(), i.to_string())),
            "every field carries its own `@iri`, and two that end alike are two fields",
        );
    }

    /// `/docs/design/labels`'s declaration: attributes stacked above a field
    /// and above the type, `@annotation` beside `@iri`, a language-tagged
    /// object, and a blank line between two fields — one `FIELD` each, every
    /// attribute a child of what it describes.
    #[test]
    fn stacked_attributes_belong_to_the_field_under_them() {
        let src = "@iri(\"https://schema.org/Person\")\n\
                   @annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"Nobel laureate\"@en)\n\
                   type Laureate\n    \
                   @iri(\"https://schema.org/name\")\n    \
                   name: Seq<LangString>\n\
                   \n    \
                   @iri(\"https://schema.org/birthDate\")\n    \
                   @annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"birth date\"@en)\n    \
                   @annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"fecha\")\n    \
                   birthDate: Date\n";
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
        let root = parse_str(src);
        assert_eq!(root.text().to_string(), src, "the tree is lossless");
        let decl = find_first_kind(&root, SyntaxKind::TYPE_DECL)
            .and_then(TypeDecl::cast)
            .expect("a TYPE_DECL");
        assert_eq!(decl.name().as_deref(), Some("Laureate"));
        let own: Vec<_> = decl
            .annotations()
            .map(|a| (a.predicate_text(), a.object_value()))
            .collect();
        assert_eq!(
            own,
            [(
                Some("http://www.w3.org/2000/01/rdf-schema#label".into()),
                Some(("Nobel laureate".into(), Some("en".into())))
            )]
        );
        let fields: Vec<_> = decl
            .fields()
            .map(|f| {
                (
                    f.name().unwrap_or_default().to_string(),
                    f.iri_attr()
                        .and_then(|a| a.iri())
                        .unwrap_or_default()
                        .to_string(),
                    f.annotations()
                        .filter_map(|a| a.object_value())
                        .map(|(v, t)| (v.to_string(), t.map(|t| t.to_string())))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            [
                (
                    "name".to_string(),
                    "https://schema.org/name".to_string(),
                    vec![]
                ),
                (
                    "birthDate".to_string(),
                    "https://schema.org/birthDate".to_string(),
                    vec![
                        ("birth date".to_string(), Some("en".to_string())),
                        ("fecha".to_string(), None)
                    ]
                ),
            ]
        );
    }

    /// `@iri` is one per field; a second is refused by name.
    #[test]
    fn a_second_iri_is_refused() {
        let src = "@iri(\"https://e.org/T\")\n\
                   type T\n    \
                   @iri(\"https://e.org/a\")\n    \
                   @iri(\"https://e.org/b\")\n    \
                   a: String\n";
        assert_eq!(
            messages(src),
            [Problem::DuplicateAttribute {
                attribute: "@iri".to_string(),
                name: "a".to_string()
            }]
        );
    }

    /// The constraints design's declaration (`/docs/design/declared`): an
    /// `@assert` above the type, one on a field with a severity, a
    /// `@severity` on a field, and a `Seq` with both bounds — every attribute
    /// a child of what it describes, the condition an `EXPR`.
    #[test]
    fn asserts_severities_and_bounds_belong_to_what_they_describe() {
        let src = "@iri(\"https://schema.org/Person\")\n\
                   @assert(alta <= baja)\n\
                   type Persona\n    \
                   @iri(\"https://schema.org/age\")\n    \
                   @assert(edad >= 0 and edad < 150)\n    \
                   edad: Option<Integer>\n\
                   \n    \
                   @iri(\"https://schema.org/email\")\n    \
                   @assert(str.matches(email, \"^[^@]+@[^@]+$\"), severity = warning)\n    \
                   @severity(info)\n    \
                   email: Seq<String, 1, 3>\n    \
                   @iri(\"https://ex.org/alta\") alta: Date\n    \
                   @iri(\"https://ex.org/baja\") baja: Option<Date>\n";
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
        let root = parse_str(src);
        assert_eq!(root.text().to_string(), src, "the tree is lossless");
        let decl = find_first_kind(&root, SyntaxKind::TYPE_DECL)
            .and_then(TypeDecl::cast)
            .expect("a TYPE_DECL");
        let on_type: Vec<_> = decl
            .asserts()
            .map(|a| a.condition().map(|c| c.text().to_string()))
            .collect();
        assert_eq!(on_type, [Some("alta <= baja".to_string())]);
        let fields: Vec<Field> = decl.fields().collect();
        let email = &fields[1];
        let assert = email.asserts().next().expect("the field's assert");
        assert_eq!(
            assert.condition().map(|c| c.text().to_string()).as_deref(),
            Some("str.matches(email, \"^[^@]+@[^@]+$\")")
        );
        assert_eq!(
            assert.severity().map(|t| t.text().to_string()).as_deref(),
            Some("warning")
        );
        assert_eq!(
            email
                .severity()
                .and_then(|s| s.level())
                .map(|t| t.text().to_string())
                .as_deref(),
            Some("info")
        );
        let ty = email.type_expr().expect("a type");
        let bounds: Vec<String> = ty.bounds().map(|t| t.text().to_string()).collect();
        assert_eq!(bounds, ["1", "3"]);
        assert_eq!(
            ty.argument().and_then(|a| a.name()).as_deref(),
            Some("String")
        );
        assert!(fields[0].severity().is_none());
    }

    /// `@severity` says how a field's count is enforced; above a type it is in
    /// the wrong place, and a field takes one.
    #[test]
    fn a_severity_is_one_per_field_and_never_on_a_type() {
        let on_type = "@iri(\"https://e.org/T\")\n\
                       @severity(warning)\n\
                       type T\n    \
                       @iri(\"https://e.org/a\") a: String\n";
        assert!(
            matches!(
                messages(on_type).as_slice(),
                [Problem::MisplacedAttribute { attribute, .. }] if attribute == "@severity"
            ),
            "got {:?}",
            messages(on_type)
        );
        let twice = "@iri(\"https://e.org/T\")\n\
                     type T\n    \
                     @iri(\"https://e.org/a\")\n    \
                     @severity(warning) @severity(info)\n    \
                     a: String\n";
        assert_eq!(
            messages(twice),
            [Problem::DuplicateAttribute {
                attribute: "@severity".to_string(),
                name: "a".to_string()
            }]
        );
    }

    /// `@assert` takes one named argument, `severity`; another name is refused
    /// by name.
    #[test]
    fn an_assert_takes_only_a_severity() {
        let src = "@iri(\"https://e.org/T\")\n\
                   type T\n    \
                   @iri(\"https://e.org/a\")\n    \
                   @assert(a > 0, level = warning)\n    \
                   a: Integer\n";
        assert!(
            matches!(
                messages(src).as_slice(),
                [Problem::UnknownParameter { parameter, .. }] if parameter == "level"
            ),
            "got {:?}",
            messages(src)
        );
    }

    /// An annotation alone is not an IRI: the field still needs one.
    #[test]
    fn an_annotation_does_not_stand_for_the_iri() {
        let src = "@iri(\"https://e.org/T\")\n\
                   type T\n    \
                   @annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"a\") a: String\n";
        assert_eq!(
            messages(src),
            [Problem::MissingIri {
                name: "a".to_string()
            }]
        );
    }

    /// An `@annotation` above an import is refused as `@iri` is.
    #[test]
    fn an_annotation_above_an_import_is_refused() {
        let src = "@annotation(\"http://www.w3.org/2000/01/rdf-schema#label\", \"x\")\n\
                   type { Person } := io.shex(\"m.shex\")\n";
        assert_eq!(
            messages(src),
            [Problem::MisplacedAttribute {
                attribute: "@annotation".to_string(),
                place: "above a `type` import".to_string(),
            }],
        );
    }

    /// `Option<Seq<String>>`: the argument of a `TYPE_EXPR` is a `TYPE_EXPR`,
    /// and `>>` closes two because it is two `GT` tokens — there is no `>>`.
    #[test]
    fn a_type_expression_nests() {
        let src = "@iri(\"https://e.org/T\")\n\
                   type T\n    \
                   @iri(\"https://e.org/tags\") tags: Option<Seq<String>>\n";
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
        let root = parse_str(src);
        let field = find_first_kind(&root, SyntaxKind::FIELD)
            .and_then(Field::cast)
            .expect("a FIELD");
        let names: Vec<String> = std::iter::successors(field.type_expr(), TypeExpr::argument)
            .filter_map(|t| t.name())
            .map(|n| n.to_string())
            .collect();
        assert_eq!(names, ["Option", "Seq", "String"]);
    }

    /// A field with no `@iri` is refused by name — `IriAttr` is mandatory in
    /// the production — and the fields after it still parse.
    #[test]
    fn a_field_without_its_iri_is_refused_and_the_next_one_parses() {
        let src = "@iri(\"https://e.org/T\")\n\
                   type T\n    \
                   name: String\n    \
                   @iri(\"https://e.org/born\") born: Date\n";
        assert_eq!(
            messages(src),
            [Problem::MissingIri {
                name: "name".to_string()
            }],
        );
        let root = parse_str(src);
        let decl = find_first_kind(&root, SyntaxKind::TYPE_DECL).expect("a TYPE_DECL");
        assert_eq!(
            decl.children()
                .filter(|c| c.kind() == SyntaxKind::FIELD)
                .count(),
            2,
        );
    }

    /// And a type with no `@iri` is the same refusal, naming the type.
    #[test]
    fn a_type_without_its_iri_is_refused() {
        let src = "type T\n    @iri(\"https://e.org/born\") born: Date\n";
        assert_eq!(
            messages(src),
            [Problem::MissingIri {
                name: "T".to_string()
            }],
        );
        assert!(descendant_kind_exists(
            &parse_str(src),
            SyntaxKind::TYPE_DECL
        ));
    }

    /// `@subject` at top level is a KNOWN name in the WRONG place, and the
    /// message says which place it belongs in. That is the whole of rule 6:
    /// one token, two names, position decides.
    ///
    /// It is consumed whole, so the import under it still parses and the line
    /// is reported once.
    #[test]
    fn rule6_subject_at_top_level_names_the_position_it_belongs_in() {
        let src = "@subject = \"x\"\ntype { Person } := io.shex(\"m.shex\")\n";
        assert_eq!(
            messages(src),
            [Problem::MisplacedAttribute {
                attribute: "@subject".to_string(),
                place: "at the top level".to_string(),
            }],
        );
        assert!(descendant_kind_exists(
            &parse_str(src),
            SyntaxKind::TYPE_DEF
        ));
    }

    /// An import takes its IRIs from the document, so an `@iri` above one is
    /// refused, and the import parses.
    #[test]
    fn rule6_an_iri_above_an_import_is_refused() {
        let src = "@iri(\"https://e.org/T\")\ntype { Person } := io.shex(\"m.shex\")\n";
        assert_eq!(
            messages(src),
            [Problem::MisplacedAttribute {
                attribute: "@iri".to_string(),
                place: "above a `type` import".to_string(),
            }],
        );
        assert!(descendant_kind_exists(
            &parse_str(src),
            SyntaxKind::TYPE_DEF
        ));
    }

    /// The same in a declaration's body: a field's attribute is `@iri`.
    #[test]
    fn rule6_subject_before_a_field_is_refused() {
        let src = "@iri(\"https://e.org/T\")\ntype T\n    @subject(\"x\") a: String\n";
        assert_eq!(
            messages(src),
            [Problem::MisplacedAttribute {
                attribute: "@subject".to_string(),
                place: "before a field of a `type` declaration".to_string(),
            }],
        );
    }

    /// And an `@iri` with no declaration under it is refused rather than
    /// swallowed: an imported type takes its IRI from the document.
    #[test]
    fn rule6_an_iri_with_no_declaration_under_it_is_refused() {
        let src = "@iri(\"https://e.org/T\")\nUser := io.csv(\"u.csv\")\n";
        let msgs = messages(src);
        assert!(
            msgs.contains(&Problem::MisplacedAttribute {
                attribute: "@iri".to_string(),
                place: "anywhere but above a `type` declaration".to_string(),
            }),
            "expected the missing-declaration refusal, got {msgs:?}",
        );
    }

    // ── RULE 7 — `as` is CONTEXTUAL ───────────────────────────────────

    /// The self-join, from `docs/programs/self-join/`. The alias binds a
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

    /// The import, by local name: `Place` is the shape's name in the document
    /// and `City` the program's. One `IMPORT_MEMBER` per member, with or
    /// without its `as`.
    #[test]
    fn rule7_an_import_member_names_its_shape_with_as() {
        let src = "type { Place as City, Person } := io.shex(\"places.shex\")\n";
        assert!(messages(src).is_empty(), "got {:?}", messages(src));
        let root = parse_str(src);
        let type_def = find_first_kind(&root, SyntaxKind::TYPE_DEF).expect("a TYPE_DEF");
        let members: Vec<(String, String)> = type_def
            .children()
            .filter_map(ImportMember::cast)
            .map(|m| {
                (
                    m.shape_name().unwrap_or_default().to_string(),
                    m.name().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            members,
            [("Place", "City"), ("Person", "Person")].map(|(s, n)| (s.to_string(), n.to_string())),
        );
    }

    /// An `as` with nothing after it names nothing.
    #[test]
    fn rule7_an_as_with_no_name_after_it_is_refused() {
        let src = "type { Place as } := io.shex(\"places.shex\")\n";
        assert_eq!(
            messages(src),
            [Problem::ExpectedToken {
                expected: "IDENT".to_string(),
                found: "RBRACE".to_string(),
            }],
        );
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
}
