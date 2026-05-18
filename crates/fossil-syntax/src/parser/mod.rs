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

pub mod diag;
pub(crate) mod expr;
pub(crate) mod items;
pub(crate) mod recover;

use diag::ParseDiagnostic;
use salsa::Accumulator;

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
    // RESEARCH.md §Q11: parser is pure (not Salsa-tracked), so it cannot
    // call `.accumulate(db)` itself. Drain its internal `Vec<ParseDiagnostic>`
    // here, inside the wrapping Salsa query, so each parse error reaches
    // the host (CLI / LSP / WASM) via the `Diagnostic` accumulator.
    for d in std::mem::take(&mut parser.diagnostics) {
        d.to_diagnostic().accumulate(db);
    }
    let green = parser.builder.finish();
    Cst::new(db, CstRoot::new(green))
}

// =====================================================================
// Internal recursive-descent parser
// =====================================================================

pub(crate) struct Parser {
    tokens: Vec<LexedToken>,
    pos: usize,
    pub(crate) builder: GreenNodeBuilder<'static>,
    /// Parser-internal diagnostic queue. Drained by the wrapping `parse()`
    /// Salsa query (RESEARCH.md §Q11) into the public `Diagnostic`
    /// accumulator after CST construction. Public to `super::recover` and
    /// `super::items` so the recovery helpers can push diagnostics without
    /// going through a `impl Parser { … }` shim per call site.
    pub(crate) diagnostics: Vec<ParseDiagnostic>,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<LexedToken>) -> Self {
        Self {
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Kind of the token at `pos`, or `None` at EOF. Does NOT skip trivia.
    pub(crate) fn current(&self) -> Option<SyntaxKind> {
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    /// Emit the current token into the green-tree builder and advance.
    pub(crate) fn bump(&mut self) {
        if let Some(t) = self.tokens.get(self.pos) {
            self.builder.token(FossilLang::kind_to_raw(t.kind), &t.text);
            self.pos += 1;
        }
    }

    /// Skip over WHITESPACE/NEWLINE/COMMENT/ERROR tokens (emitting them as
    /// trivia into the green tree to keep the CST lossless).
    pub(crate) fn skip_trivia(&mut self) {
        while matches!(
            self.current(),
            Some(SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT)
        ) {
            self.bump();
        }
    }

    /// Lookahead at `offset` non-trivia tokens past `pos`. Returns `None` at EOF.
    pub(crate) fn peek_kind(&self, offset: usize) -> Option<SyntaxKind> {
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

    /// True iff the next `n` non-trivia tokens are contiguous in the source
    /// (no WHITESPACE / NEWLINE / COMMENT between them). Used by the
    /// PrefixedName disambiguator (grammar.bnf line 226: `IDENT SHAPE_SEP
    /// LocalName` requires NO whitespace between `IDENT` and `:`).
    pub(crate) fn peek_contiguous(&self, n: usize) -> bool {
        let mut found = 0;
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if matches!(
                t.kind,
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            ) {
                if found < n {
                    return false;
                }
                return true;
            }
            found += 1;
            if found >= n {
                return true;
            }
            i += 1;
        }
        found >= n
    }

    pub(crate) fn start(&mut self, kind: SyntaxKind) {
        self.builder.start_node(FossilLang::kind_to_raw(kind));
    }

    pub(crate) fn finish(&mut self) {
        self.builder.finish_node();
    }

    /// Record a checkpoint into the green-tree builder. Used by the Pratt
    /// expression sub-parser (`parser::expr`) to wrap an already-emitted
    /// sub-tree (the LHS atom) inside an infix/postfix composite node.
    pub(crate) fn checkpoint(&self) -> rowan::Checkpoint {
        self.builder.checkpoint()
    }

    /// Start a node retroactively at `cp`, wrapping every token / node
    /// emitted between the checkpoint and now.
    pub(crate) fn start_at(&mut self, cp: rowan::Checkpoint, kind: SyntaxKind) {
        self.builder
            .start_node_at(cp, FossilLang::kind_to_raw(kind));
    }

    /// Eat the current token if it matches `kind`; otherwise emit it under an
    /// ERROR node. Phase 1 has no diagnostic emission yet — the ERROR node
    /// is the recovery signal for downstream and a TODO marker for Phase 2.
    pub(crate) fn expect(&mut self, kind: SyntaxKind) {
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

    pub(crate) fn bump_as_error(&mut self) {
        self.start(SyntaxKind::ERROR);
        if self.pos < self.tokens.len() {
            self.bump();
        }
        self.finish();
    }

    /// Byte-offset start of the current non-trivia token, or end-of-input
    /// span at EOF. Used by `recover::recover_to` and
    /// `recover::expect_or_recover` to attach precise spans to
    /// [`diag::ParseDiagnostic`]s.
    pub(crate) fn current_token_span_start(&self) -> usize {
        // Walk forward over trivia to find the current real token, mirroring
        // `current()` / `peek_kind(0)` semantics. If nothing remains, fall
        // back to the end of the input (well-defined for EOF diagnostics).
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if !matches!(
                t.kind,
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            ) {
                return t.range.start;
            }
            i += 1;
        }
        self.tokens.last().map_or(0, |t| t.range.end)
    }

    /// Append a `ParseDiagnostic` to the parser's internal queue. Drained
    /// by the wrapping Salsa `parse()` query (see `parse` above) into the
    /// public `Diagnostic` accumulator after CST construction.
    pub(crate) fn push_diagnostic(&mut self, d: ParseDiagnostic) {
        self.diagnostics.push(d);
    }

    // --- top-level -------------------------------------------------------
    //
    // Phase 2 plan 02-03 moved every per-item recursive-descent rule out of
    // this file into `super::items`. `Parser::parse_program` stays as a
    // thin shim so the unit tests inside this module (and the legacy
    // `parse_text` helper) keep their original API surface.

    fn parse_program(&mut self) {
        items::parse_program(self);
    }

    // --- expressions -----------------------------------------------------
    //
    // Phase 2: expression parsing is delegated to the Pratt sub-parser in
    // `parser::expr` (per RESEARCH.md §Q1 + Crafting Interpreters Ch. 17).
    // The item parser keeps a thin `parse_expr` wrapper that wraps the
    // expression in an `EXPR` node for back-compat with Phase 1 callers
    // (`parse_property`, etc.) that expect the outer node kind.

    pub(crate) fn parse_expr(&mut self) {
        self.start(SyntaxKind::EXPR);
        self.skip_trivia();
        expr::parse_expression(self, 0);
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
