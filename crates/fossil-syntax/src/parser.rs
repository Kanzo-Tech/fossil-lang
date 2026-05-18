//! Hand-rolled recursive-descent parser building a lossless rowan CST.
//!
//! The Salsa-tracked entry point is [`parse`]; it wraps the parser internals
//! and returns a [`Cst`] whose `root` accessor yields the [`SyntaxNode`].
//!
//! # Phase 1 grammar subset (productions)
//!
//! ```text
//! Program       := (PrefixDecl | SourceDef | Mapping)*
//! PrefixDecl    := KW_PREFIX IDENT SHAPE_SEP ABS_IRI
//! SourceDef     := IDENT DEFINE CallExpr
//! CallExpr      := IDENT SHAPE_SEP IDENT LPAREN STRING RPAREN
//! Mapping       := MappingHeader INDENT MappingBody DEDENT
//! MappingHeader := IDENT SHAPE_SEP IDENT SHAPE_SEP IDENT KW_FROM IDENT
//! MappingBody   := Property+
//! Property      := PropertyLhs ASSIGN Expr
//! PropertyLhs   := IDENT | IDENT SHAPE_SEP IDENT
//! Expr          := TEMPLATE | DOT IDENT | STRING | IDENT (SHAPE_SEP IDENT)?
//! ```
//!
//! Phase 2+ adds operator precedence, ternaries, annotations, error recovery.

use rowan::{GreenNode, GreenNodeBuilder, Language};

use crate::indent::{LexedToken, lex_with_indents};
use crate::kind::{FossilLang, SyntaxKind, SyntaxNode};

/// Salsa-storable handle to a parsed CST.
///
/// Wraps a [`rowan::GreenNode`] (which is `Send + Sync` thanks to its internal
/// `Arc`) and reconstructs the red [`SyntaxNode`] view on demand via
/// [`CstRoot::syntax`]. This is the canonical rust-analyzer / ruff-db / ty
/// pattern: green trees are the storage layer; red trees are recreated
/// cheaply per access.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CstRoot {
    green: GreenNode,
}

impl CstRoot {
    #[must_use]
    pub const fn new(green: GreenNode) -> Self {
        Self { green }
    }

    /// Reconstruct the red [`SyntaxNode`] view. Cheap (`Arc::clone` internally).
    #[must_use]
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// Direct access to the underlying [`GreenNode`] for callers that need it.
    #[must_use]
    pub const fn green(&self) -> &GreenNode {
        &self.green
    }
}

// SAFETY: third-party-trait integration boundary (per ADR-0004).
// Salsa's `Update` trait is `unsafe` by design — implementations must
// guarantee that `maybe_update` correctly determines whether the new value
// differs from the old (used to invalidate downstream queries). We delegate
// to `PartialEq` on `GreenNode`, which rowan implements as structural tree
// equality. No safe alternative exists because Salsa requires `unsafe impl`
// even for trivially-safe bodies.
#[allow(unsafe_code)]
unsafe impl salsa::Update for CstRoot {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: caller guarantees `old_pointer` is a valid, aligned pointer
        // to an initialised `CstRoot` owned by Salsa storage (Salsa contract).
        let old = unsafe { &mut *old_pointer };
        if *old == new_value {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

/// Salsa-tracked CST handle. Construct via [`parse`].
///
/// Carries a `'db` lifetime tying it to the [`fossil_base::Db`] it was parsed
/// from; downstream queries (HIR lowering, etc.) consume it as `Cst<'db>`.
#[salsa::tracked(debug)]
pub struct Cst<'db> {
    #[returns(ref)]
    pub root: CstRoot,
}

/// Parse `file` into a lossless CST. Salsa-tracked: re-parsing the same file
/// is a memo hit; mutating `file.text(db)` invalidates and re-parses.
///
/// Phase 2-9 contract: this signature is locked. New diagnostics flow via
/// the [`fossil_base::Diagnostic`] accumulator (added in Phase 2).
#[salsa::tracked]
pub fn parse(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> Cst<'_> {
    let text = file.text(db);
    let tokens = lex_with_indents(text);
    let mut parser = Parser::new(tokens);
    parser.parse_program();
    let green = parser.builder.finish();
    Cst::new(db, CstRoot::new(green))
}

// =====================================================================
// Internal recursive-descent parser
// =====================================================================

struct Parser {
    tokens: Vec<LexedToken>,
    pos: usize,
    builder: GreenNodeBuilder<'static>,
}

impl Parser {
    fn new(tokens: Vec<LexedToken>) -> Self {
        Self {
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
        }
    }

    /// Kind of the token at `pos`, or `None` at EOF. Does NOT skip trivia.
    fn current(&self) -> Option<SyntaxKind> {
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    /// Emit the current token into the green-tree builder and advance.
    fn bump(&mut self) {
        if let Some(t) = self.tokens.get(self.pos) {
            self.builder.token(FossilLang::kind_to_raw(t.kind), &t.text);
            self.pos += 1;
        }
    }

    /// Skip over WHITESPACE/NEWLINE/COMMENT/ERROR tokens (emitting them as
    /// trivia into the green tree to keep the CST lossless).
    fn skip_trivia(&mut self) {
        while matches!(
            self.current(),
            Some(SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT)
        ) {
            self.bump();
        }
    }

    /// Lookahead at `offset` non-trivia tokens past `pos`. Returns `None` at EOF.
    fn peek_kind(&self, offset: usize) -> Option<SyntaxKind> {
        let mut depth = 0;
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if !matches!(
                t.kind,
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            ) {
                if depth == offset {
                    return Some(t.kind);
                }
                depth += 1;
            }
            i += 1;
        }
        None
    }

    fn start(&mut self, kind: SyntaxKind) {
        self.builder.start_node(FossilLang::kind_to_raw(kind));
    }

    fn finish(&mut self) {
        self.builder.finish_node();
    }

    /// Eat the current token if it matches `kind`; otherwise emit it under an
    /// ERROR node. Phase 1 has no diagnostic emission yet — the ERROR node
    /// is the recovery signal for downstream and a TODO marker for Phase 2.
    fn expect(&mut self, kind: SyntaxKind) {
        self.skip_trivia();
        if self.current() == Some(kind) {
            self.bump();
        } else {
            self.start(SyntaxKind::ERROR);
            if self.pos < self.tokens.len() {
                self.bump();
            }
            self.finish();
        }
    }

    fn bump_as_error(&mut self) {
        self.start(SyntaxKind::ERROR);
        if self.pos < self.tokens.len() {
            self.bump();
        }
        self.finish();
    }

    // --- top-level -------------------------------------------------------

    fn parse_program(&mut self) {
        self.start(SyntaxKind::PROGRAM);
        loop {
            self.skip_trivia();
            // Drop stray virtual tokens at the top level (DEDENTs after the
            // last mapping reach here).
            if matches!(
                self.current(),
                Some(SyntaxKind::INDENT | SyntaxKind::DEDENT)
            ) {
                self.bump();
                continue;
            }
            match self.current() {
                None => break,
                Some(SyntaxKind::KW_PREFIX) => self.parse_prefix_decl(),
                Some(SyntaxKind::IDENT) => match self.peek_kind(1) {
                    Some(SyntaxKind::DEFINE) => self.parse_source_def(),
                    Some(SyntaxKind::SHAPE_SEP) => self.parse_mapping(),
                    _ => self.bump_as_error(),
                },
                _ => self.bump_as_error(),
            }
        }
        self.finish();
    }

    // --- prefix decl -----------------------------------------------------

    fn parse_prefix_decl(&mut self) {
        self.start(SyntaxKind::PREFIX_DECL);
        self.expect(SyntaxKind::KW_PREFIX);
        self.expect(SyntaxKind::IDENT);
        self.expect(SyntaxKind::SHAPE_SEP);
        self.expect(SyntaxKind::ABS_IRI);
        self.finish();
    }

    // --- source def ------------------------------------------------------

    fn parse_source_def(&mut self) {
        self.start(SyntaxKind::SOURCE_DEF);
        self.expect(SyntaxKind::IDENT);
        self.expect(SyntaxKind::DEFINE);
        self.parse_call_expr();
        self.finish();
    }

    fn parse_call_expr(&mut self) {
        self.start(SyntaxKind::CALL_EXPR);
        // Callee: IDENT (SHAPE_SEP IDENT)?  e.g. `io.csv` is parsed as
        // IDENT DOT IDENT, but `io:csv` would be IDENT SHAPE_SEP IDENT.
        // hello.fossil uses `io.csv("...")`, so accept the dotted form too.
        self.expect(SyntaxKind::IDENT);
        self.skip_trivia();
        if matches!(
            self.current(),
            Some(SyntaxKind::DOT | SyntaxKind::SHAPE_SEP)
        ) {
            self.bump();
            self.expect(SyntaxKind::IDENT);
        }
        self.expect(SyntaxKind::LPAREN);
        self.expect(SyntaxKind::STRING);
        self.expect(SyntaxKind::RPAREN);
        self.finish();
    }

    // --- mapping ---------------------------------------------------------

    fn parse_mapping(&mut self) {
        self.start(SyntaxKind::MAPPING);
        self.parse_mapping_header();
        self.skip_trivia();
        if self.current() == Some(SyntaxKind::INDENT) {
            self.bump();
            self.parse_mapping_body();
            self.skip_trivia();
            if self.current() == Some(SyntaxKind::DEDENT) {
                self.bump();
            }
        }
        self.finish();
    }

    fn parse_mapping_header(&mut self) {
        // `User : ex:Person from users`
        self.start(SyntaxKind::MAPPING_HEADER);
        self.expect(SyntaxKind::IDENT); // User
        self.expect(SyntaxKind::SHAPE_SEP); // :
        self.expect(SyntaxKind::IDENT); // ex
        self.expect(SyntaxKind::SHAPE_SEP); // :
        self.expect(SyntaxKind::IDENT); // Person
        self.expect(SyntaxKind::KW_FROM); // from
        self.expect(SyntaxKind::IDENT); // users
        self.finish();
    }

    fn parse_mapping_body(&mut self) {
        self.start(SyntaxKind::MAPPING_BODY);
        loop {
            self.skip_trivia();
            match self.current() {
                // `iri` keyword OR a bare/prefixed IDENT starts a Property
                // (grammar.bnf line 141: `PropertyLhs := 'iri' | IRIExpr`).
                Some(SyntaxKind::IDENT | SyntaxKind::KW_IRI) => self.parse_property(),
                _ => break,
            }
        }
        self.finish();
    }

    fn parse_property(&mut self) {
        self.start(SyntaxKind::PROPERTY);
        self.parse_property_lhs();
        self.skip_trivia();
        self.expect(SyntaxKind::ASSIGN);
        self.parse_expr();
        self.finish();
    }

    fn parse_property_lhs(&mut self) {
        self.start(SyntaxKind::PROPERTY_LHS);
        self.skip_trivia();
        match self.current() {
            // `iri = ...` — the `iri` keyword as the PropertyLhs literal.
            Some(SyntaxKind::KW_IRI) => self.bump(),
            // Bare IDENT, optionally followed by `: IDENT` for a prefixed name.
            Some(SyntaxKind::IDENT) => {
                self.bump();
                self.skip_trivia();
                if self.current() == Some(SyntaxKind::SHAPE_SEP) {
                    self.bump();
                    self.expect(SyntaxKind::IDENT);
                }
            }
            _ => {
                // Wrong shape entirely — emit ERROR so the body loop can break.
                self.bump_as_error();
            }
        }
        self.finish();
    }

    // --- expressions (Phase 1: tiny) -------------------------------------

    fn parse_expr(&mut self) {
        self.start(SyntaxKind::EXPR);
        self.skip_trivia();
        match self.current() {
            Some(SyntaxKind::TEMPLATE) => {
                self.start(SyntaxKind::TEMPLATE_EXPR);
                self.bump();
                self.finish();
            }
            Some(SyntaxKind::STRING) => {
                self.start(SyntaxKind::LITERAL_EXPR);
                self.bump();
                self.finish();
            }
            Some(SyntaxKind::ABS_IRI) => {
                self.start(SyntaxKind::IRI_EXPR);
                self.bump();
                self.finish();
            }
            Some(SyntaxKind::DOT) => {
                self.start(SyntaxKind::FIELD_REF_EXPR);
                self.bump();
                self.expect(SyntaxKind::IDENT);
                self.finish();
            }
            Some(SyntaxKind::IDENT) => {
                // Could be a bare IDENT or a prefixed name `ex:foo`.
                self.start(SyntaxKind::LITERAL_EXPR);
                self.bump();
                self.skip_trivia();
                if self.current() == Some(SyntaxKind::SHAPE_SEP) {
                    self.bump();
                    self.expect(SyntaxKind::IDENT);
                }
                self.finish();
            }
            _ => self.bump_as_error(),
        }
        self.finish();
    }
}

// =====================================================================
// Parser tests (unit-level — full integration test in lib.rs)
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_text(input: &str) -> SyntaxNode {
        let tokens = lex_with_indents(input);
        let mut p = Parser::new(tokens);
        p.parse_program();
        SyntaxNode::new_root(p.builder.finish())
    }

    #[test]
    fn parses_lone_prefix_decl() {
        let root = parse_text("prefix ex: <https://example.org/>\n");
        assert_eq!(root.kind(), SyntaxKind::PROGRAM);
        let kinds: Vec<_> = root.children().map(|c| c.kind()).collect();
        assert_eq!(kinds, vec![SyntaxKind::PREFIX_DECL]);
    }

    #[test]
    fn parses_source_def_with_dotted_callee() {
        let root = parse_text("users := io.csv(\"users.csv\")\n");
        let kinds: Vec<_> = root.children().map(|c| c.kind()).collect();
        assert_eq!(kinds, vec![SyntaxKind::SOURCE_DEF]);
    }

    #[test]
    fn parses_mapping_with_two_properties() {
        let input = "User : ex:Person from users\n    iri = `${.id}`\n    ex:name = .name\n";
        let root = parse_text(input);
        let mapping = root
            .children()
            .find(|n| n.kind() == SyntaxKind::MAPPING)
            .expect("expected a MAPPING child");
        let body = mapping
            .children()
            .find(|n| n.kind() == SyntaxKind::MAPPING_BODY)
            .expect("expected a MAPPING_BODY child");
        let props: Vec<_> = body
            .children()
            .filter(|n| n.kind() == SyntaxKind::PROPERTY)
            .collect();
        assert_eq!(props.len(), 2);
    }
}
