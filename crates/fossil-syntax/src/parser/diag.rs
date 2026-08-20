//! Parser-internal diagnostic enum.
//!
//! The parser is a pure helper (NOT a Salsa-tracked
//! function), so it cannot call `.accumulate(db)` directly. Instead the
//! parser collects `ParseDiagnostic`s in a `Vec` and the wrapping
//! `parse(db, file)` Salsa query iterates them after CST construction,
//! accumulating each via `to_diagnostic().accumulate(db)`.
//!
//! Five variants: the three the Pratt expression parser and the full item
//! parser needed, `UnlexableCharacter` for the byte the
//! lexer has no rule for, and [`ParseDiagnostic::RetiredSpelling`] for a form
//! this parser RECOGNISES and refuses. The `to_diagnostic` adapter keeps the
//! parser decoupled from the public `Diagnostic` shape.
//!
//! # Why a retired spelling gets its own variant
//!
//! The other four say what the parser wanted. A retired spelling is the case
//! where the parser knows exactly what the author wrote AND exactly what to
//! write instead, and `expected IDENT, found SHAPE_SEP` throws both away. This
//! matters more here than it usually would: the measured failure mode in this
//! tree is a RETURN WITHOUT A WORD — `lower_property` ends in `return None` and
//! `body::body` skips it in silence, so a program half-written in the new
//! spelling compiled and lost the work. A rejection that names the form is what
//! keeps the next step from reading a false green.

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
    /// A byte no lexer rule matched, named. Distinct from `UnexpectedToken`
    /// because there is no token to name: the message has to quote the source
    /// text, and it is the only diagnostic a file like `#` produces at all.
    UnlexableCharacter { text: String, span: Span },
    /// INDENT/DEDENT inconsistency surfaced by the indent pass.
    InconsistentDedent { span: Span },
    /// A spelling `grammar.bnf` retired, recognised on purpose so the message
    /// can name what replaces it. `span` covers the whole retired form, not one
    /// token of it: the author has to see the thing being refused.
    RetiredSpelling { message: String, span: Span },
    /// A form the grammar HAS, written incompletely, where naming the missing
    /// token would say less than naming the production.
    ///
    /// `@subject` where `@rename` goes and a `Rename` with no `as` are both
    /// this: the parser knows which production it is in and can quote it, while
    /// `expected IDENT, found STRING` throws that away. It is the sibling of
    /// [`ParseDiagnostic::RetiredSpelling`] and not the same thing — that one
    /// refuses a form the language no longer has, this one refuses an
    /// incomplete instance of a form it does.
    Malformed { message: String, span: Span },
}

impl ParseDiagnostic {
    /// Translate the parser-internal diagnostic into a public `Diagnostic`
    /// suitable for `.accumulate(db)` inside the `parse` Salsa query.
    #[must_use]
    pub fn to_diagnostic(self) -> Diagnostic {
        match self {
            // Parse diagnostics carry neither a structured suggestion source
            // nor a did-you-mean candidate (Markdown-only hints today). Those
            // structured fields belong to the `ShEx` `OneOf`-rejection emitter
            // and the did-you-mean quick-fix, which have a replacement to
            // propose; a parse error has only a position. Built via the
            // `Diagnostic::new` builder so future field additions stay
            // non-breaking here.
            Self::ExpectedToken { want, got, span } => Diagnostic::new(
                Severity::Error,
                format!("expected {want:?}, found {got:?}"),
                span,
            ),
            Self::UnexpectedToken { span } => {
                Diagnostic::new(Severity::Error, "unexpected token", span)
            }
            Self::UnlexableCharacter { text, span } => Diagnostic::new(
                Severity::Error,
                format!("unexpected character `{text}` — no token starts with it"),
                span,
            ),
            Self::InconsistentDedent { span } => {
                Diagnostic::new(Severity::Error, "inconsistent indentation", span)
            }
            Self::RetiredSpelling { message, span } | Self::Malformed { message, span } => {
                Diagnostic::new(Severity::Error, message, span)
            }
        }
    }
}

/// The six retired spellings, their messages, and the one place they are
/// written down.
///
/// Each is a tombstone in `grammar.bnf` — a form the file declares ABSENT and
/// names no production for, so there is nothing to cite. They live here
/// rather than at the ten call sites so the wording is one thing: the corpus
/// rewrite of step 8 reads these strings, and a message that drifts between two
/// call sites is a message the corpus cannot pin.
pub mod retired {
    /// There is no `PrefixDecl`, and `prefix` is an ordinary identifier.
    pub const PREFIX_DECL: &str = "`prefix` declares a vocabulary, and there is no vocabulary \
         left to declare: the CURIE is gone from every position it held. Write a full IRI inside \
         a string — `\"http://xmlns.com/foaf/0.1/name\"` — and a bare name everywhere else.";

    /// There is no `PrefixedName`: a `:` that is not a mapping header or a
    /// ternary is an error.
    pub const CURIE: &str = "a `:` here is neither a mapping header nor a ternary, so `prefix:Local` \
         is not a name fossil has. A shape is one of the names a `type { … } := …` binding \
         introduced, and a property is the last segment of the predicate IRI the shape declares — \
         both bare.";

    /// There is no `ABS_IRI`, so `<` has one reading and never opens anything.
    pub const ABSOLUTE_IRI: &str = "an absolute IRI in `<…>` is not a form fossil has: `<` is the \
         comparison operator and nothing else. A constant IRI is a string — \
         `\"http://xmlns.com/foaf/0.1/name\"` — and the shape decides that it denotes rather than \
         reads.";

    /// There is no `FieldRef`: a leading `.` is an error.
    pub const LEADING_DOT: &str = "a leading `.` names a column of a row with no name, and the row \
         has a name now. Every reference is qualified: write `User.name`, naming the row this \
         mapping draws `from`.";

    /// There is no `TEMPLATE`: `${`, a backtick and `\$` are not tokens.
    pub const BACKTICK: &str = "a backtick opens nothing: it was a second spelling of the quoted \
         string, differing only in the delimiter and in `${` for the hole. Write \
         `\"…{expr}…\"` — one spelling, and the hole is an ordinary expression.";

    /// There is no `PIPE`: `|>` is not a token.
    pub const PIPELINE: &str = "`|>` was a second spelling of the member call and ruling 7 of \
         2026-08-11 retired it: with members resolved by the type of the receiver, the pipeline \
         had nothing left that the dot could not do. Write `a.f(…)` — the receiver goes to the \
         left of the dot, where every other member call already puts it.";
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
    fn unlexable_character_names_the_character() {
        let d = ParseDiagnostic::UnlexableCharacter {
            text: "#".to_string(),
            span: Span::new(0, 1),
        };
        let out = d.to_diagnostic();
        assert!(matches!(out.severity, Severity::Error));
        assert!(
            out.message.contains('#'),
            "the character must be in the message: {}",
            out.message,
        );
        assert_eq!(out.span.start, 0);
        assert_eq!(out.span.end, 1);
    }

    #[test]
    fn a_retired_spelling_keeps_its_message_and_its_span() {
        let d = ParseDiagnostic::RetiredSpelling {
            message: retired::CURIE.to_string(),
            span: Span::new(7, 16),
        };
        let out = d.to_diagnostic();
        assert!(matches!(out.severity, Severity::Error));
        // The span covers the FORM, not one token of it — a `:` underlined on
        // its own says nothing about what was written.
        assert_eq!((out.span.start, out.span.end), (7, 16));
        assert!(out.message.contains("bare"), "{}", out.message);
    }

    #[test]
    fn every_retired_message_says_what_to_write_instead() {
        // A refusal that only says «no» costs the author a search through a
        // grammar they do not have. Each of the five names its replacement.
        for m in [
            retired::PREFIX_DECL,
            retired::CURIE,
            retired::ABSOLUTE_IRI,
            retired::LEADING_DOT,
            retired::BACKTICK,
        ] {
            assert!(
                m.contains('`'),
                "a retired-spelling message must quote the form that replaces it: {m}"
            );
        }
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
