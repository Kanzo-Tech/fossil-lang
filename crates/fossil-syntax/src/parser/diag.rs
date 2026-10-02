//! Parser-internal diagnostic enum.
//!
//! The parser is a pure helper (NOT a Salsa-tracked
//! function), so it cannot call `.accumulate(db)` directly. Instead the
//! parser collects `ParseDiagnostic`s in a `Vec` and the wrapping
//! `parse(db, file)` Salsa query iterates them after CST construction,
//! accumulating each via `to_diagnostic().accumulate(db)`.
//!
//! The variants split by what the parser can SAY: what it wanted, the byte the
//! lexer has no rule for ([`ParseDiagnostic::UnlexableCharacter`]), and the two
//! forms it RECOGNISES and refuses ([`ParseDiagnostic::RetiredSpelling`],
//! [`ParseDiagnostic::Malformed`]). The `to_diagnostic` adapter keeps the
//! parser decoupled from the public `Diagnostic` shape.
//!
//! # Why a retired spelling gets its own variant
//!
//! The others say what the parser wanted. A retired spelling is the case
//! where the parser knows exactly what the author wrote AND exactly what to
//! write instead, and `expected IDENT, found SHAPE_SEP` throws both away. This
//! matters more here than it usually would: the measured failure mode in this
//! tree is a RETURN WITHOUT A WORD — `lower_property` ends in `return None` and
//! `body::body` skips it in silence, so a program half-written in the new
//! spelling compiled and lost the work. A rejection that names the form is what
//! keeps the next step from reading a false green.

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
    /// A spelling `grammar.bnf` retired, recognised on purpose so the message
    /// can name what replaces it. `span` covers the whole retired form, not one
    /// token of it: the author has to see the thing being refused.
    RetiredSpelling {
        form: &'static retired::Retired,
        span: Span,
    },
    /// A form the grammar HAS, written incompletely, where naming the missing
    /// token would say less than naming the production.
    ///
    /// `@subject` where `@rename` goes and a `Rename` with no `as` are both
    /// this: the parser knows which production it is in and can quote it, while
    /// `expected IDENT, found STRING` throws that away. It is the sibling of
    /// [`ParseDiagnostic::RetiredSpelling`] and not the same thing — that one
    /// refuses a form the language no longer has, this one refuses an
    /// incomplete instance of a form it does.
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
            Self::RetiredSpelling { form, span } => Diagnostic::new(
                Severity::Error,
                Problem::RetiredSpelling {
                    spelling: form.spelling.to_string(),
                    replacement: form.replacement.to_string(),
                },
                span,
            )
            .with_help(form.why),
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

/// The six retired spellings and the one place they are written down.
///
/// Each is a tombstone in `grammar.bnf` — a form the file declares ABSENT and
/// names no production for, so there is nothing to cite. They live here
/// rather than at the ten call sites so the wording is one thing.
pub mod retired {
    /// A retired form: what it was written as, what replaces it, and why it went.
    #[derive(Debug, PartialEq, Eq)]
    pub struct Retired {
        /// The form as the author wrote it.
        pub spelling: &'static str,
        /// What to write instead.
        pub replacement: &'static str,
        /// Why the form is gone; the diagnostic's help.
        pub why: &'static str,
    }

    /// There is no `PrefixDecl`, and `prefix` is an ordinary identifier.
    pub const PREFIX_DECL: Retired = Retired {
        spelling: "prefix",
        replacement: "\"<full IRI>\"",
        why: "`prefix` declares a vocabulary, and there is no vocabulary left to declare: the \
              CURIE is gone from every position it held. Write a full IRI inside a string — \
              `\"http://xmlns.com/foaf/0.1/name\"` — and a bare name everywhere else.",
    };

    /// There is no `PrefixedName`: a `:` that is not a mapping header or a
    /// ternary is an error.
    pub const CURIE: Retired = Retired {
        spelling: "prefix:Local",
        replacement: "Local",
        why: "a `:` here is neither a mapping header nor a ternary, so `prefix:Local` is not a \
              name fossil has. A shape is one of the names a `type { … } := …` binding \
              introduced, and a property is the last segment of the predicate IRI the shape \
              declares — both bare.",
    };

    /// There is no `ABS_IRI`, so `<` has one reading and never opens anything.
    pub const ABSOLUTE_IRI: Retired = Retired {
        spelling: "<…>",
        replacement: "\"http://…\"",
        why: "an absolute IRI in `<…>` is not a form fossil has: `<` is the comparison operator \
              and nothing else. A constant IRI is a string — \
              `\"http://xmlns.com/foaf/0.1/name\"` — and the shape decides that it denotes \
              rather than reads.",
    };

    /// There is no `FieldRef`: a leading `.` is an error.
    pub const LEADING_DOT: Retired = Retired {
        spelling: ".name",
        replacement: "Row.name",
        why: "a leading `.` names a column of a row with no name, and the row has a name now. \
              Every reference is qualified: write `User.name`, naming the row this mapping \
              draws `from`.",
    };

    /// There is no `TEMPLATE`: `${`, a backtick and `\$` are not tokens.
    pub const BACKTICK: Retired = Retired {
        spelling: "`…`",
        replacement: "\"…{expr}…\"",
        why: "a backtick opens nothing: it was a second spelling of the quoted string, differing \
              only in the delimiter and in `${` for the hole. Write `\"…{expr}…\"` — one \
              spelling, and the hole is an ordinary expression. A column whose name is not an \
              identifier is quoted the same way after the dot: `Row.\"Person.id\"`.",
    };

    /// There is no `PIPE`: `|>` is not a token.
    pub const PIPELINE: Retired = Retired {
        spelling: "|>",
        replacement: "a.f(…)",
        why: "`|>` was a second spelling of the member call and it is retired: with members \
              resolved by the type of the receiver, the pipeline had nothing left that the dot \
              could not do. Write `a.f(…)` — the receiver goes to the left of the dot, where \
              every other member call already puts it.",
    };
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

    #[test]
    fn a_retired_spelling_keeps_its_form_its_reason_and_its_span() {
        let d = ParseDiagnostic::RetiredSpelling {
            form: &retired::CURIE,
            span: Span::new(7, 16),
        };
        let out = d.to_diagnostic();
        assert!(matches!(out.severity, Severity::Error));
        // The span covers the FORM, not one token of it — a `:` underlined on
        // its own says nothing about what was written.
        assert_eq!((out.span.start, out.span.end), (7, 16));
        assert_eq!(
            out.problem,
            Problem::RetiredSpelling {
                spelling: "prefix:Local".to_string(),
                replacement: "Local".to_string(),
            }
        );
        assert!(out.help.as_deref().is_some_and(|h| h.contains("bare")));
    }

    #[test]
    fn every_retired_form_says_what_to_write_instead() {
        // A refusal that only says «no» costs the author a search through a
        // grammar they do not have. Each names its replacement.
        for form in [
            &retired::PREFIX_DECL,
            &retired::CURIE,
            &retired::ABSOLUTE_IRI,
            &retired::LEADING_DOT,
            &retired::BACKTICK,
            &retired::PIPELINE,
        ] {
            assert!(!form.replacement.is_empty(), "{form:?}");
            assert_ne!(form.spelling, form.replacement, "{form:?}");
        }
    }
}
