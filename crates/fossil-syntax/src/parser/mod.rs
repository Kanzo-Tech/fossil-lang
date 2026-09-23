//! Hand-rolled recursive-descent parser building a lossless rowan CST.
//!
//! The Salsa-tracked entry point is [`parse`]; it wraps the parser internals
//! and returns a [`Cst`] whose `root` accessor yields the [`SyntaxNode`].
//!
//! # What this parser accepts
//!
//! ```text
//! Program       := TopLevel*
//! TopLevel      := SourceDef | MultiSourceDef | TypeDef | Mapping
//! SourceDef     := IDENT DEFINE Expression
//! MultiSourceDef:= LBRACE IDENT (COMMA IDENT)* RBRACE DEFINE Expression
//! TypeDef       := RenameAttr* 'type'
//!                  LBRACE IDENT (COMMA IDENT)* RBRACE DEFINE Expression
//! RenameAttr    := AT_ATTR LPAREN IDENT (COMMA Rename)+ RPAREN
//! Rename        := STRING 'as' IDENT
//! Mapping       := MappingHeader INDENT MappingBody DEDENT
//! MappingHeader := IDENT SHAPE_SEP ShapeExpr KW_FROM Expression
//! ShapeExpr     := IDENT
//! MappingBody   := (SubjectAssign | Property)*         ← the grammar says
//!                                                        `SubjectAssign
//!                                                        Property+`; the three
//!                                                        obligations are facts
//!                                                        about a mapping and
//!                                                        `fossil_hir::body`
//!                                                        checks them
//! SubjectAssign := AT_ATTR ASSIGN Expression
//! Property      := PropertyLhs ASSIGN Expression
//! PropertyLhs   := IDENT
//! ```
//!
//! One difference from `grammar.bnf` remains, and it is the one marked inline
//! above: the grammar says `MappingBody := SubjectAssign Property+` and this
//! parser accepts `(SubjectAssign | Property)*`, because the obligations on the
//! identity are facts about a mapping rather than about its spelling and
//! `fossil_hir::body` is what checks them. Everything else in the table above
//! is the specification verbatim.
//!
//! # What this parser REFUSES, by name
//!
//! Six spellings the grammar retired are recognised on purpose rather than
//! left to fall through the recovery arms, because the parser knows what was
//! written and what replaces it. Their messages live in [`diag::retired`]:
//! `prefix ex: <…>`, the CURIE `ex:name`, the `<…>` absolute IRI, a leading `.`,
//! the backtick and `|>`. A refusal that says `expected IDENT, found SHAPE_SEP`
//! throws away both halves of that.
//!
//! Expressions are the Pratt sub-parser in [`expr`]; its own header carries the
//! accounting for `|>`.

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

// SAFETY: third-party-trait integration boundary — the workspace denies
// `unsafe_code` rather than forbidding it, so a boundary like this one opts in
// with an explicit `allow` and this justification.
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
/// This signature is locked. New diagnostics flow via the
/// [`fossil_base::Diagnostic`] accumulator, never by widening the return type.
#[salsa::tracked]
pub fn parse(db: &dyn fossil_base::Db, file: fossil_base::SourceFile) -> Cst<'_> {
    let text = file.text(db);
    let tokens = lex_with_indents(text);
    let mut parser = Parser::new(tokens);
    parser.parse_program();
    // The parser is pure (not Salsa-tracked), so it cannot
    // call `.accumulate(db)` itself. Drain its internal `Vec<ParseDiagnostic>`
    // here, inside the wrapping Salsa query, so each parse error reaches
    // the host (CLI / LSP / WASM) via the `Diagnostic` accumulator.
    // `.file_absolute()` and not the default frame: `parse` is FILE-keyed, so a
    // parse error's span is a file offset by construction. Left in the default
    // `MappingRelative`, the per-mapping drain in the native host rebases it by
    // the mapping's own start — which moves the copy, so the dedup keyed on
    // (severity, message, span) stops collapsing the two and the reader gets the
    // same error twice, the second one pointing past the end of the file.
    for d in std::mem::take(&mut parser.diagnostics) {
        d.to_diagnostic().file_absolute().accumulate(db);
    }
    let green = parser.builder.finish();
    Cst::new(db, CstRoot::new(green))
}

// =====================================================================
// Internal recursive-descent parser
// =====================================================================

/// How far [`Parser::retire`] consumes. See its doc comment for the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetiredRun {
    /// Exactly N non-trivia tokens.
    Count(usize),
    /// The rest of the line.
    Line,
}

pub(crate) struct Parser {
    tokens: Vec<LexedToken>,
    pos: usize,
    pub(crate) builder: GreenNodeBuilder<'static>,
    /// Parser-internal diagnostic queue. Drained by the wrapping `parse()`
    /// Salsa query into the public `Diagnostic`
    /// accumulator after CST construction. Public to `super::recover` and
    /// `super::items` so the recovery helpers can push diagnostics without
    /// going through a `impl Parser { … }` shim per call site.
    pub(crate) diagnostics: Vec<ParseDiagnostic>,
    /// How many ternary THEN-branches are open above the current position.
    ///
    /// # This is the contiguity check, and it is not the same thing
    ///
    /// Disambiguation rule 3 (grammar.bnf, § DISAMBIGUATION RULES) retires the
    /// no-whitespace-before-the-colon rule, and it is right to: with no
    /// `PrefixedName` left, a `:` in an expression
    /// has exactly one reading and the parser needs nothing to find it. What
    /// the tombstone does not account for is that REFUSING `ex:name` by name is
    /// not free — to say «this is a CURIE and CURIEs are gone» you have to know
    /// it is not the `:` of `cond ? a : b`, which is the very question the
    /// deleted rule answered.
    ///
    /// So the check comes back one layer up, as a fact the parser already has
    /// rather than a fact about whitespace: inside a then-branch a `:` belongs
    /// to the ternary, and `expr::parse_primary` does not offer its refusal
    /// there. Outside one, `IDENT : IDENT` is the dead spelling and nothing
    /// else — which is what lets `cond ? a:b` parse clean, spaces or no spaces,
    /// where the old rule made it a prefixed name.
    ternary_then_depth: usize,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<LexedToken>) -> Self {
        Self {
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            diagnostics: Vec::new(),
            ternary_then_depth: 0,
        }
    }

    /// Run `f` with one more ternary then-branch open. See
    /// [`Parser::ternary_then_depth`]; [`Parser::in_ternary_then`] reads it.
    pub(crate) fn inside_ternary_then(&mut self, f: impl FnOnce(&mut Self)) {
        self.ternary_then_depth += 1;
        f(self);
        self.ternary_then_depth -= 1;
    }

    /// Is a `:` at the current position the ternary's rather than a CURIE's?
    pub(crate) const fn in_ternary_then(&self) -> bool {
        self.ternary_then_depth > 0
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

    /// Skip over WHITESPACE/NEWLINE/COMMENT tokens (emitting them into the
    /// green tree to keep the CST lossless).
    ///
    /// `ERROR` is NOT in that set and must never join it. An `ERROR` token is
    /// either the indent pass's inconsistent dedent or a byte the lexer had no
    /// rule for; skipping it as trivia would put the byte in the tree and the
    /// diagnostic nowhere, which is the bug this parser had until the lexer
    /// stopped dropping those bytes altogether.
    pub(crate) fn skip_trivia(&mut self) {
        while matches!(
            self.current(),
            Some(SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT)
        ) {
            self.bump();
        }
    }

    /// The text of the current non-trivia token. `None` at EOF.
    ///
    /// Only contextual keywords need this: `type` is NOT reserved (it is a
    /// plausible column name in a language whose
    /// commonest predicate is `rdf:type`), so `type {` is told from any other
    /// `IDENT {` by the identifier's spelling and nothing else.
    pub(crate) fn current_text(&self) -> Option<&str> {
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if !matches!(
                t.kind,
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            ) {
                return Some(t.text.as_str());
            }
            i += 1;
        }
        None
    }

    /// Lookahead at `offset` non-trivia tokens past `pos`. Returns `None` at EOF.
    pub(crate) fn peek_kind(&self, offset: usize) -> Option<SyntaxKind> {
        self.peek(offset).map(|t| t.kind)
    }

    /// The TEXT of the token `offset` non-trivia tokens ahead.
    ///
    /// [`Parser::current_text`] is `peek_text(0)` and both exist for the same
    /// reason: a contextual keyword is told from an identifier by its spelling
    /// and nothing else. `current_text` covers `type` at the start of an item;
    /// this one covers `as`, which is contextual in the MIDDLE of a form —
    /// `"…" as name` and `Node as Other` — so the decision needs a token the
    /// parser has not reached yet.
    pub(crate) fn peek_text(&self, offset: usize) -> Option<&str> {
        self.peek(offset).map(|t| t.text.as_str())
    }

    /// The `offset`-th non-trivia token past `pos`, or `None` at EOF.
    fn peek(&self, offset: usize) -> Option<&LexedToken> {
        let mut depth = 0;
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if !matches!(
                t.kind,
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            ) {
                if depth == offset {
                    return Some(t);
                }
                depth += 1;
            }
            i += 1;
        }
        None
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
    /// `ERROR` node AND push an [`diag::ParseDiagnostic::ExpectedToken`] into
    /// the parser's queue. The diagnostic is drained into the public
    /// `Diagnostic` accumulator by the wrapping [`parse`] Salsa query.
    ///
    /// Prefer [`recover::expect_or_recover`] with an explicit anchor set: the
    /// recovery cascade is local to the caller, while this `expect` consumes
    /// the offending token and may eat an anchor the caller expected to see.
    pub(crate) fn expect(&mut self, kind: SyntaxKind) {
        self.skip_trivia();
        if self.current() == Some(kind) {
            self.bump();
        } else {
            let got = self.current().unwrap_or(SyntaxKind::EOF);
            let span_start = self.current_token_span_start();
            self.push_diagnostic(ParseDiagnostic::ExpectedToken {
                want: kind,
                got,
                span: fossil_base::Span::new(
                    u32::try_from(span_start).unwrap_or(u32::MAX),
                    u32::try_from(span_start).unwrap_or(u32::MAX),
                ),
            });
            self.start(SyntaxKind::ERROR);
            if self.pos < self.tokens.len() {
                self.bump();
            }
            self.finish();
        }
    }

    /// Consume the current token into a fresh `ERROR` node and push a
    /// diagnostic into the parser's queue. Used as the catch-all fallback in
    /// Pratt primary dispatch, in the item parser's malformed-LHS branches, and
    /// at the top level.
    ///
    /// The diagnostic is [`diag::ParseDiagnostic::UnlexableCharacter`] when the
    /// token being consumed is an `ERROR` carrying source text — the byte the
    /// lexer had no rule for, which [`crate::indent::lex_with_indents`]
    /// preserves precisely so this can name it. Otherwise it is
    /// [`diag::ParseDiagnostic::UnexpectedToken`], which names nothing because
    /// the span is enough: the token exists and the reader can see it.
    ///
    /// (An `ERROR` token with EMPTY text is the indent pass's inconsistent
    /// dedent, not a byte. It has nothing to quote, so it takes the generic
    /// message.)
    pub(crate) fn bump_as_error(&mut self) {
        let span_start = self.current_token_span_start();
        let tok = self.tokens.get(self.pos);
        let span_end = tok.map_or(span_start, |t| t.range.end);
        let span = fossil_base::Span::new(
            u32::try_from(span_start).unwrap_or(u32::MAX),
            u32::try_from(span_end).unwrap_or(u32::MAX),
        );
        let diagnostic = match tok {
            // The backtick is the one byte logos rejects that the language used
            // to have a token for, so `unexpected character` would be true and
            // useless: the author wrote a string, in the spelling that lost. It
            // is named here rather than in a parser arm because there IS no arm
            // — the byte never reaches a production.
            Some(t) if t.kind == SyntaxKind::ERROR && t.text == "`" => {
                ParseDiagnostic::RetiredSpelling {
                    message: diag::retired::BACKTICK.to_string(),
                    span,
                }
            }
            Some(t) if t.kind == SyntaxKind::ERROR && !t.text.is_empty() => {
                ParseDiagnostic::UnlexableCharacter {
                    text: t.text.clone(),
                    span,
                }
            }
            _ => ParseDiagnostic::UnexpectedToken { span },
        };
        self.push_diagnostic(diagnostic);
        self.start(SyntaxKind::ERROR);
        if self.pos < self.tokens.len() {
            self.bump();
        }
        self.finish();
    }

    /// How far [`Parser::retire`] consumes.
    ///
    /// Every retired spelling is CONSUMED under one `ERROR` node rather than
    /// recovered past. Recovery would leave the tokens outside the node, and the
    /// author would get `unexpected token` for a form the parser recognised
    /// exactly — the same silence, one layer up, that made `lower_property`'s
    /// bare `return None` cost a program its properties.
    ///
    /// # `RetiredRun::Count`
    ///
    /// Takes N non-trivia tokens. The CURIE is three (`ex` `:` `name`); a
    /// leading `.` is two.
    ///
    /// # `RetiredRun::Line`
    ///
    /// Takes the rest of the line, COMMENT included. `prefix ex: <…>` is a
    /// whole statement and underlining a prefix of it would point at a
    /// plausible wrong place.
    ///
    /// There was a third, `Through(kind)` — «up to and including the first `>`»
    /// — written for the absolute IRI and deleted when it was measured. It can
    /// never find its `>`: `//` in the IRI's scheme is the comment opener, and
    /// with `ABS_IRI` gone nothing claims it first, so `<https://example.org/>`
    /// lexes as `< https :` and then a COMMENT that runs to the line break.
    /// That is why the comment counts toward the span here rather than being
    /// skipped as the trivia it is — the bytes are the form being refused.
    pub(crate) fn retire(&mut self, message: &str, run: RetiredRun) {
        self.skip_trivia();
        let start = self.current_token_span_start();
        let mut end = start;
        let mut left = match run {
            RetiredRun::Count(n) => n,
            RetiredRun::Line => usize::MAX,
        };
        self.start(SyntaxKind::ERROR);
        while left > 0 {
            let Some(t) = self.tokens.get(self.pos) else {
                break;
            };
            match t.kind {
                // A line break ends every retired form: none of them spans one,
                // and running past it would swallow the next item.
                SyntaxKind::NEWLINE => break,
                SyntaxKind::WHITESPACE => {}
                // A comment is trivia everywhere else and is part of the form
                // here: see the note above on `//`.
                SyntaxKind::COMMENT => end = t.range.end,
                _ => {
                    left -= 1;
                    end = t.range.end;
                }
            }
            self.bump();
        }
        self.finish();
        self.push_diagnostic(ParseDiagnostic::RetiredSpelling {
            message: message.to_string(),
            span: fossil_base::Span::new(
                u32::try_from(start).unwrap_or(u32::MAX),
                u32::try_from(end).unwrap_or(u32::MAX),
            ),
        });
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
    // Every per-item recursive-descent rule lives in `super::items`, not here.
    // `Parser::parse_program` stays as a thin shim so the unit tests inside
    // this module (and the `parse_text` helper) keep one entry point.

    fn parse_program(&mut self) {
        items::parse_program(self);
    }

    // --- expressions -----------------------------------------------------
    //
    // Expression parsing is delegated to the Pratt sub-parser in
    // `parser::expr` (Crafting Interpreters Ch. 17).
    // The item parser keeps a thin `parse_expr` wrapper that wraps the
    // expression in an `EXPR` node, because callers like `parse_property`
    // locate the right-hand side by that outer node kind.

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

    fn diagnose_text(input: &str) -> Vec<String> {
        let tokens = lex_with_indents(input);
        let mut p = Parser::new(tokens);
        p.parse_program();
        p.diagnostics
            .iter()
            .cloned()
            .map(|d| d.to_diagnostic().message)
            .collect()
    }

    /// The retired vocabulary declaration is REFUSED, and the refusal names the
    /// spelling that replaces it. Not `expected IDENT, found IDENT` — the parser
    /// knows what this line is.
    #[test]
    fn a_prefix_declaration_is_refused_by_name() {
        let msgs = diagnose_text("prefix ex: <https://example.org/>\n");
        assert_eq!(msgs.len(), 1, "one refusal for one retired line: {msgs:?}");
        assert!(msgs[0].contains("no vocabulary"), "{}", msgs[0]);
    }

    /// And `prefix` is an ordinary identifier again
    /// (grammar.bnf, § RESERVED KEYWORDS): the
    /// refusal above is triggered by the SHAPE of the line, not by the word. A
    /// word reserved for a form the language does not have costs every program
    /// that wanted it as a column name.
    #[test]
    fn prefix_is_still_a_usable_binding_name() {
        let root = parse_text("prefix := io.csv(\"p.csv\")\n");
        let kinds: Vec<_> = root.children().map(|c| c.kind()).collect();
        assert_eq!(kinds, vec![SyntaxKind::SOURCE_DEF]);
        assert!(diagnose_text("prefix := io.csv(\"p.csv\")\n").is_empty());
    }

    #[test]
    fn parses_source_def_with_dotted_callee() {
        let root = parse_text("users := io.csv(\"users.csv\")\n");
        let kinds: Vec<_> = root.children().map(|c| c.kind()).collect();
        assert_eq!(kinds, vec![SyntaxKind::SOURCE_DEF]);
    }

    #[test]
    fn parses_mapping_with_two_properties() {
        let input =
            "Users : Person from User\n    @subject = \"u/{User.id}\"\n    name = User.name\n";
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
