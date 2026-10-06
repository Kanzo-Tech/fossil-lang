//! Parser-internal diagnostic enum.
//!
//! The parser is a pure helper (NOT a Salsa-tracked
//! function), so it cannot call `.accumulate(db)` directly. Instead the
//! parser collects `ParseDiagnostic`s in a `Vec` and the wrapping
//! `parse(db, file)` Salsa query iterates them after CST construction,
//! accumulating each via `to_diagnostic().accumulate(db)`.
//!
//! The variants split by what the parser can SAY: what it wanted, the byte the
//! lexer has no rule for ([`ParseDiagnostic::UnlexableCharacter`]), and an
//! incomplete production it recognises ([`ParseDiagnostic::Malformed`]). The
//! `to_diagnostic` adapter keeps the parser decoupled from the public
//! `Diagnostic` shape.

use fossil_base::{Diagnostic, Problem, Severity, Span};

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
    /// A byte no lexer rule matched, named. Distinct from `UnexpectedToken`
    /// because there is no token to name: the message has to quote the source
    /// text, and it is the only diagnostic a file like `#` produces at all.
    UnlexableCharacter { text: String, span: Span },
    /// A form the grammar HAS, written incompletely, where naming the missing
    /// token would say less than naming the production.
    ///
    /// `@subject` where `@rename` goes and a `Rename` with no `as` are both
    /// this: the parser knows which production it is in and can quote it, while
    /// `expected IDENT, found STRING` throws that away.
    Malformed {
        problem: Problem,
        help: Option<String>,
        span: Span,
    },
}

impl ParseDiagnostic {
    /// Translate the parser-internal diagnostic into a public `Diagnostic`
    /// suitable for `.accumulate(db)` inside the `parse` Salsa query.
    #[must_use]
    pub fn to_diagnostic(self) -> Diagnostic {
        match self {
            // A parse error has only a position — no replacement to propose —
            // so no arm below sets `did_you_mean` or `suggestion_source`.
            Self::ExpectedToken { want, got, span } => Diagnostic::new(
                Severity::Error,
                Problem::ExpectedToken {
                    expected: format!("{want:?}"),
                    found: format!("{got:?}"),
                },
                span,
            ),
            Self::UnexpectedToken { span } => {
                Diagnostic::new(Severity::Error, Problem::UnexpectedToken {}, span)
            }
            Self::UnlexableCharacter { text, span } => Diagnostic::new(
                Severity::Error,
                Problem::UnknownCharacter { character: text },
                span,
            ),
            Self::Malformed {
                problem,
                help,
                span,
            } => {
                let d = Diagnostic::new(Severity::Error, problem, span);
                match help {
                    Some(help) => d.with_help(help),
                    None => d,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_token_carries_both_kinds() {
        let d = ParseDiagnostic::ExpectedToken {
            want: SyntaxKind::KW_FROM,
            got: SyntaxKind::IDENT,
            span: Span::new(10, 14),
        };
        let out = d.to_diagnostic();
        assert!(matches!(out.severity, Severity::Error));
        assert_eq!(
            out.problem,
            Problem::ExpectedToken {
                expected: "KW_FROM".to_string(),
                found: "IDENT".to_string(),
            }
        );
        assert_eq!(out.span.start, 10);
        assert_eq!(out.span.end, 14);
    }

    #[test]
    fn unexpected_token_is_its_code() {
        let d = ParseDiagnostic::UnexpectedToken {
            span: Span::new(0, 1),
        };
        let out = d.to_diagnostic();
        assert_eq!(out.problem.code(), "syntax/unexpected-token");
    }

    #[test]
    fn unlexable_character_names_the_character() {
        let d = ParseDiagnostic::UnlexableCharacter {
            text: "#".to_string(),
            span: Span::new(0, 1),
        };
        let out = d.to_diagnostic();
        assert!(matches!(out.severity, Severity::Error));
        assert_eq!(
            out.problem,
            Problem::UnknownCharacter {
                character: "#".to_string()
            }
        );
        assert_eq!(out.span.start, 0);
        assert_eq!(out.span.end, 1);
    }
}
