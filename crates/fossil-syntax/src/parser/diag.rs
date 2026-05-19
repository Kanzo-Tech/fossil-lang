//! Parser-internal diagnostic enum.
//!
//! Per RESEARCH.md §Q11: the parser is a pure helper (NOT a Salsa-tracked
//! function), so it cannot call `.accumulate(db)` directly. Instead the
//! parser collects `ParseDiagnostic`s in a `Vec` and the wrapping
//! `parse(db, file)` Salsa query iterates them after CST construction,
//! accumulating each via `to_diagnostic().accumulate(db)`.
//!
//! Phase 2 ships three variants — the minimum surface that plans 02-02
//! (Pratt) and 02-03 (full item parser + recovery) need to emit. Later
//! phases extend this enum; the `to_diagnostic` adapter pattern keeps the
//! parser decoupled from the public `Diagnostic` accumulator shape.

use fossil_base::{Diagnostic, Severity, Span};

use crate::kind::SyntaxKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseDiagnostic {
    /// `expected {want:?}, found {got:?}` at `span`.
    ExpectedToken {
        want: SyntaxKind,
        got: SyntaxKind,
        span: Span,
    },
    /// Unexpected token consumed during recovery.
    UnexpectedToken { span: Span },
    /// INDENT/DEDENT inconsistency surfaced by the indent pass.
    InconsistentDedent { span: Span },
}

impl ParseDiagnostic {
    /// Translate the parser-internal diagnostic into a public `Diagnostic`
    /// suitable for `.accumulate(db)` inside the `parse` Salsa query.
    #[must_use]
    pub fn to_diagnostic(self) -> Diagnostic {
        match self {
            Self::ExpectedToken { want, got, span } => Diagnostic {
                severity: Severity::Error,
                message: format!("expected {want:?}, found {got:?}"),
                span,
                // Phase 3 plan 03-03: parse diagnostics carry no
                // code-suggestion source (Markdown-only hints today). The
                // structured field is reserved for the OneOf-rejection
                // emitter (plan 03-05) and similar future suggestion-
                // emitting diagnostics.
                suggestion_source: None,
            },
            Self::UnexpectedToken { span } => Diagnostic {
                severity: Severity::Error,
                message: "unexpected token".to_string(),
                span,
                suggestion_source: None,
            },
            Self::InconsistentDedent { span } => Diagnostic {
                severity: Severity::Error,
                message: "inconsistent indentation".to_string(),
                span,
                suggestion_source: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_token_renders_message() {
        let d = ParseDiagnostic::ExpectedToken {
            want: SyntaxKind::KW_FROM,
            got: SyntaxKind::IDENT,
            span: Span::new(10, 14),
        };
        let out = d.to_diagnostic();
        assert!(matches!(out.severity, Severity::Error));
        assert!(out.message.contains("KW_FROM"));
        assert!(out.message.contains("IDENT"));
        assert_eq!(out.span.start, 10);
        assert_eq!(out.span.end, 14);
    }

    #[test]
    fn unexpected_token_renders_generic_message() {
        let d = ParseDiagnostic::UnexpectedToken {
            span: Span::new(0, 1),
        };
        let out = d.to_diagnostic();
        assert!(out.message.contains("unexpected token"));
    }

    #[test]
    fn inconsistent_dedent_renders_message() {
        let d = ParseDiagnostic::InconsistentDedent {
            span: Span::new(20, 20),
        };
        let out = d.to_diagnostic();
        assert!(out.message.contains("inconsistent indentation"));
    }
}
