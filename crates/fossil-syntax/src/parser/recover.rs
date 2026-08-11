// See `parser::expr` for the rationale on the redundant_pub_crate allow.
// `parser::recover::recover_to` / `expect_or_recover` / the ANCHOR consts
// are called from sibling submodules (`parser::items`, `parser::expr`), so
// they need `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

//! Error recovery per RESEARCH.md §Q2: anchor-based "bump until we see a known
//! start-of-the-next-thing" recovery. Anchors are kept tiny — Fossil's INDENT/
//! DEDENT layer + the small set of item-leading keywords makes this trivial.
//!
//! Two primitives live here:
//!
//! - [`recover_to`] — consume tokens into an `ERROR` node until the current
//!   token is in `anchors` (or EOF). Emits a single `UnexpectedToken`
//!   diagnostic per call.
//! - [`expect_or_recover`] — like the Phase 1 `Parser::expect` but DOES NOT
//!   consume the offending token on mismatch. Instead it emits a zero-width
//!   `ERROR` marker + an `ExpectedToken` diagnostic, then delegates to
//!   `recover_to` so the offending token can be re-examined by the caller's
//!   recovery cascade.
//!
//! The Phase 1 `Parser::expect` was kept as a thin wrapper for back-compat
//! with the existing prefix-decl / source-def / mapping-header callers — see
//! `super::Parser::expect`. New code SHOULD prefer `expect_or_recover` with
//! an explicit anchor set.

use crate::kind::SyntaxKind;

use super::Parser;
use super::diag::ParseDiagnostic;

/// Token kinds that start a top-level item. After a parse error inside or
/// between items, the parser bumps until it sees one of these (or EOF).
///
/// `IDENT` is deliberately included even though it is ambiguous at
/// lookahead-0 — `parse_program` re-disambiguates IDENT into `SourceDef` /
/// `Mapping` / fallthrough-error on the next iteration.
pub(crate) const TOP_LEVEL_ANCHORS: &[SyntaxKind] = &[SyntaxKind::KW_PREFIX, SyntaxKind::IDENT];

/// Token kinds that end a property inside a `MAPPING_BODY`: either the
/// `NEWLINE` separator or the `DEDENT` closing the body block. Note
/// `peek_kind` skips `NEWLINE` as trivia, so for property-level recovery the
/// practical anchor is `DEDENT` plus whatever starts the next property
/// (`IDENT` / `KW_IRI`).
pub(crate) const MAPPING_BODY_ANCHORS: &[SyntaxKind] =
    &[SyntaxKind::DEDENT, SyntaxKind::IDENT, SyntaxKind::KW_IRI];

// There was a `CLOSE_BRACKET_ANCHORS` here — `RBRACE` / `RPAREN` / `DEDENT`,
// for recovering inside a `{ … }` annotation block. The annotation block was
// its only caller and it went with the form.

/// Consume tokens into an `ERROR` node until the current token is in
/// `anchors` (or EOF). The anchor token itself is NOT consumed — it remains
/// the current token so the caller can resume normal parsing.
///
/// Emits one [`ParseDiagnostic::UnexpectedToken`] per call (spanning the run
/// of consumed tokens). Multi-token recovery runs still produce a single
/// diagnostic — by design, to avoid diagnostic spam during a single recovery
/// event.
///
/// If the current token already matches an anchor, this is a no-op (no
/// `ERROR` node, no diagnostic).
///
/// # No-progress invariant — CALLER RESPONSIBILITY
///
/// `recover_to` is **deliberately no-op** when the current token is already
/// in `anchors` (the "we're already at a known recovery point" fast path).
/// This means a caller that sits **inside an outer `loop`** and dispatches
/// based on the same anchor set must NEVER fall through to `recover_to`
/// for a kind that is in its own anchor set — the loop would re-enter the
/// same arm with the same token forever (the parser hangs at 100% CPU).
///
/// When the caller cannot itself advance past an unexpected token (because
/// the unexpected kind is *in* the anchor set — typically `IDENT` in
/// `TOP_LEVEL_ANCHORS`), it MUST use [`super::Parser::bump_as_error`]
/// instead. `bump_as_error` ALWAYS consumes exactly one token under an
/// ERROR node, guaranteeing `p.pos` advances monotonically.
///
/// See `crates/fossil-syntax/src/parser/items.rs::parse_program` for the
/// canonical example of the safe shape: when the IDENT lookahead doesn't
/// match `DEFINE` or `SHAPE_SEP`, the fall-through is `p.bump_as_error()`,
/// NOT `recover_to(p, TOP_LEVEL_ANCHORS)`. Historical bug:
/// `.planning/phases/06-cli-complete-lsp/deferred-items.md` (parser-hang on
/// a bare `#` — the logos lexer drops `#` silently, the next non-trivia
/// token is often IDENT, and `IDENT ∈ TOP_LEVEL_ANCHORS` made the call a
/// no-op).
pub(crate) fn recover_to(p: &mut Parser, anchors: &[SyntaxKind]) {
    p.skip_trivia();
    if at_anchor(p, anchors) {
        return;
    }
    let span_start = p.current_token_span_start();
    p.start(SyntaxKind::ERROR);
    while !at_anchor(p, anchors) {
        p.bump();
        p.skip_trivia();
    }
    let span_end = p.current_token_span_start();
    p.finish();
    p.push_diagnostic(ParseDiagnostic::UnexpectedToken {
        span: fossil_base::Span::new(
            u32::try_from(span_start).unwrap_or(u32::MAX),
            u32::try_from(span_end).unwrap_or(u32::MAX),
        ),
    });
}

/// Like `Parser::expect(want)` but on mismatch:
///
/// 1. Emit [`ParseDiagnostic::ExpectedToken`] at the current span.
/// 2. Emit a zero-width `ERROR` node so the CST carries the structural marker.
/// 3. DO NOT consume the offending token — fall through to [`recover_to`]
///    which bumps to a known anchor.
///
/// This is the structural fix the Phase 1 [`Parser::expect`] lacks. The Phase
/// 1 implementation consumed the wrong token on mismatch, which broke the
/// recovery cascade because the anchor token disappeared into an `ERROR`.
pub(crate) fn expect_or_recover(p: &mut Parser, want: SyntaxKind, anchors: &[SyntaxKind]) {
    p.skip_trivia();
    if p.current() == Some(want) {
        p.bump();
        return;
    }
    let got = p.current().unwrap_or(SyntaxKind::EOF);
    let span_start = p.current_token_span_start();
    p.push_diagnostic(ParseDiagnostic::ExpectedToken {
        want,
        got,
        span: fossil_base::Span::new(
            u32::try_from(span_start).unwrap_or(u32::MAX),
            u32::try_from(span_start).unwrap_or(u32::MAX),
        ),
    });
    // Emit a zero-width ERROR node so the CST has the structural marker even
    // when the offending token is left for downstream recovery.
    p.start(SyntaxKind::ERROR);
    p.finish();
    // Caller is responsible for choosing the anchor set; we apply it here so
    // every `expect_or_recover` site is one-call-fits-all.
    recover_to(p, anchors);
}

/// `true` iff the current non-trivia token is in `anchors`, OR the stream is
/// at EOF (`None`). EOF is an implicit anchor of every recovery context.
fn at_anchor(p: &Parser, anchors: &[SyntaxKind]) -> bool {
    p.current().is_none_or(|k| anchors.contains(&k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indent::lex_with_indents;

    fn parser(src: &str) -> Parser {
        Parser::new(lex_with_indents(src))
    }

    #[test]
    fn recover_to_stops_at_anchor() {
        // `garbage prefix ex: <x>` — anchor on KW_PREFIX. After recovery the
        // current token MUST be `prefix`.
        let mut p = parser("garbage prefix ex: <x>");
        // Open a synthetic PROGRAM node so the green-tree builder is in a
        // valid `start_node` context for the ERROR node `recover_to` emits.
        p.start(SyntaxKind::PROGRAM);
        recover_to(&mut p, &[SyntaxKind::KW_PREFIX]);
        assert_eq!(p.current(), Some(SyntaxKind::KW_PREFIX));
        p.finish();
    }

    #[test]
    fn recover_to_emits_one_diagnostic_per_call() {
        let mut p = parser("garbage prefix ex: <x>");
        p.start(SyntaxKind::PROGRAM);
        recover_to(&mut p, &[SyntaxKind::KW_PREFIX]);
        assert_eq!(
            p.diagnostics.len(),
            1,
            "expected exactly one UnexpectedToken diagnostic for a single recover_to call",
        );
        assert!(matches!(
            p.diagnostics[0],
            ParseDiagnostic::UnexpectedToken { .. }
        ));
        p.finish();
    }

    #[test]
    fn recover_to_noop_when_already_at_anchor() {
        let mut p = parser("prefix ex: <x>");
        p.start(SyntaxKind::PROGRAM);
        recover_to(&mut p, &[SyntaxKind::KW_PREFIX]);
        assert_eq!(
            p.diagnostics.len(),
            0,
            "no diagnostic when already at anchor"
        );
        assert_eq!(p.current(), Some(SyntaxKind::KW_PREFIX));
        p.finish();
    }

    #[test]
    fn expect_or_recover_does_not_consume_on_mismatch() {
        // Input is `Foo` (an IDENT); we expect KW_FROM. After mismatch the
        // current token MUST still be the IDENT — recovery is the caller's
        // concern (we pass an empty anchor set so recover_to is a no-op
        // because Foo is not in the set AND there is no following token to
        // bump-stop at, the loop ends only at EOF).
        let mut p = parser("Foo");
        p.start(SyntaxKind::PROGRAM);
        // Use an anchor set containing IDENT itself so recover_to bails
        // immediately after expect_or_recover emits its ERROR marker.
        expect_or_recover(&mut p, SyntaxKind::KW_FROM, &[SyntaxKind::IDENT]);
        assert_eq!(
            p.current(),
            Some(SyntaxKind::IDENT),
            "offending token must NOT be consumed by expect_or_recover",
        );
        assert_eq!(p.diagnostics.len(), 1);
        assert!(matches!(
            p.diagnostics[0],
            ParseDiagnostic::ExpectedToken { .. }
        ));
        p.finish();
    }

    #[test]
    fn expect_or_recover_consumes_on_match() {
        let mut p = parser("Foo");
        p.start(SyntaxKind::PROGRAM);
        expect_or_recover(&mut p, SyntaxKind::IDENT, &[]);
        assert_eq!(p.current(), None, "matching token consumed; stream at EOF");
        assert_eq!(p.diagnostics.len(), 0, "no diagnostic on match");
        p.finish();
    }
}
