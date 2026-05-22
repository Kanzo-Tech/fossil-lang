//! Salsa accumulator for diagnostics.
//!
//! Any `#[salsa::tracked]` query can build a [`Diagnostic`] via the struct
//! literal or via [`Diagnostic::new`] and call `.accumulate(db)`; the host
//! (CLI / LSP / WASM) collects them via
//! `query::accumulated::<Diagnostic>(db, input)`.
//!
//! ## `suggestion_source` field (Phase 3 plan 03-03 — Blocker #3 fix)
//!
//! `Diagnostic` carries an optional `suggestion_source: Option<String>` —
//! a STRUCTURED carrier for code-suggestion source text emitted alongside
//! the diagnostic. Phase 3's `ShEx` `OneOf`-rejection emitter (plan 03-05)
//! populates this with `fossil_descriptors_output::generate_split_suggestion`
//! output; plan 03-08's `shex_one_of_split_suggestion_compiles` second-order
//! test reads the field directly and feeds it through `parse -> lower ->
//! typecheck_mapping` to assert "the suggestion compiles".
//!
//! Why structured (not Markdown-message-substring): type-safe, future-proof
//! for other suggestion-emitting diagnostics, and avoids brittle string
//! manipulation. See ADR-0006 §Consequences.
//!
//! ## `did_you_mean` field (Phase 6 plan 06-08 — SC#5 code-actions)
//!
//! `Diagnostic` also carries an optional `did_you_mean: Option<DidYouMean>` —
//! the STRUCTURED carrier for the Levenshtein replacement candidate that
//! `fossil_hir::didyoumean::did_you_mean` (Phase 3) computes. Phase 3 surfaced
//! the candidate only inside the diagnostic message text (the "did you mean
//! ..." suffix); Phase 6's `fossil_ide::code_action` did-you-mean quick-fix
//! needs the `(wrong_span, replacement)` pair STRUCTURALLY so it can build a
//! `WorkspaceEdit` without re-parsing the message string. This mirrors the
//! `suggestion_source` precedent exactly (ADR-0006 Approach A — structured, not
//! string-parsed). Defaults to `None`; plain data (wasm-clean).

#[salsa::accumulator]
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Optional structured suggestion-source text. Populated by
    /// suggestion-emitting diagnostics (e.g. `ShEx` `OneOf` rejection's
    /// split-into-N-mappings code suggestion); read directly by consumers
    /// via the typed field, NOT via Markdown delimiter parsing. See module
    /// doc + ADR-0006.
    pub suggestion_source: Option<String>,
    /// Optional structured did-you-mean candidate. Populated by a diagnostic
    /// whose [`Severity::Error`] message proposes a Levenshtein replacement for
    /// a typo'd identifier; read directly by the `fossil_ide::code_action`
    /// did-you-mean quick-fix to build a `WorkspaceEdit` replacing
    /// [`DidYouMean::wrong_span`] with [`DidYouMean::replacement`], NOT by
    /// parsing the message text. See module doc + ADR-0006.
    pub did_you_mean: Option<DidYouMean>,
}

/// A structured did-you-mean candidate: replace the source text at
/// `wrong_span` with `replacement`.
///
/// Carried by [`Diagnostic::did_you_mean`] so the IDE did-you-mean code action
/// (Phase 6 SC#5) can build a `WorkspaceEdit` STRUCTURALLY — the `(span,
/// replacement)` pair is the exact input a single-edit quick-fix needs, with no
/// message-string parsing. Plain data → wasm-clean.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DidYouMean {
    /// The byte span of the typo'd identifier in the source — what the quick-fix
    /// replaces.
    pub wrong_span: Span,
    /// The suggested replacement text (the Levenshtein nearest candidate).
    pub replacement: String,
}

impl DidYouMean {
    /// Build a did-you-mean candidate from the typo's span + the replacement.
    #[must_use]
    pub fn new(wrong_span: Span, replacement: impl Into<String>) -> Self {
        Self {
            wrong_span,
            replacement: replacement.into(),
        }
    }
}

impl Diagnostic {
    /// Build a [`Diagnostic`] with no attached suggestion source or
    /// did-you-mean candidate.
    ///
    /// Equivalent to the struct literal with `suggestion_source: None` and
    /// `did_you_mean: None`. Use [`Self::with_suggestion_source`] /
    /// [`Self::with_did_you_mean`] to attach them fluently.
    #[must_use]
    pub fn new(severity: Severity, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity,
            message: message.into(),
            span,
            suggestion_source: None,
            did_you_mean: None,
        }
    }

    /// Attach a suggestion-source string (e.g. the generated split snippet
    /// for `ShEx` `OneOf` rejection).
    #[must_use]
    pub fn with_suggestion_source(mut self, source: impl Into<String>) -> Self {
        self.suggestion_source = Some(source.into());
        self
    }

    /// Attach a structured did-you-mean candidate (the typo's span + the
    /// Levenshtein replacement) so the IDE quick-fix reads it structurally.
    #[must_use]
    pub fn with_did_you_mean(mut self, wrong_span: Span, replacement: impl Into<String>) -> Self {
        self.did_you_mean = Some(DidYouMean::new(wrong_span, replacement));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Byte-offset span into the source text.
///
/// Phase 1 carries `(start, end)` only. Phase 2 may extend with file id
/// once cross-file spans are needed, via a `FileSpan` newtype layered above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

#[cfg(test)]
// `literal_string_with_formatting_args` flags the Fossil IRI template syntax
// (`${ex:}u/${.id}`) in the test fixture as if it were an inline-format-args
// candidate. It's literal Fossil source — not a Rust format string.
#[allow(clippy::literal_string_with_formatting_args)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_default_suggestion_source_is_none() {
        let d = Diagnostic::new(Severity::Error, "oops", Span::new(0, 4));
        assert!(d.suggestion_source.is_none());
    }

    #[test]
    fn diagnostic_with_suggestion_source_round_trips() {
        let snippet =
            "UserEmail : ex:Person from users\n    iri = `${ex:}u/${.id}`\n    ex:email = .email\n";
        let d = Diagnostic::new(Severity::Error, "ShEx OneOf", Span::new(10, 20))
            .with_suggestion_source(snippet);
        assert_eq!(d.suggestion_source.as_deref(), Some(snippet));
    }

    #[test]
    fn diagnostic_struct_literal_with_explicit_none_still_compiles() {
        // Phase-2 callers continue to work — they set the field to None
        // explicitly. This test locks that contract.
        let d = Diagnostic {
            severity: Severity::Warning,
            message: "ok".into(),
            span: Span::new(0, 0),
            suggestion_source: None,
            did_you_mean: None,
        };
        assert!(d.suggestion_source.is_none());
        assert!(d.did_you_mean.is_none());
    }

    #[test]
    fn diagnostic_default_did_you_mean_is_none() {
        let d = Diagnostic::new(Severity::Error, "oops", Span::new(0, 4));
        assert!(d.did_you_mean.is_none());
    }

    #[test]
    fn diagnostic_with_did_you_mean_carries_structured_candidate() {
        // The IDE quick-fix reads the (wrong_span, replacement) pair directly —
        // no message-string parsing (ADR-0006 Approach A).
        let d = Diagnostic::new(
            Severity::Error,
            "unknown column `naem` — did you mean `name`?",
            Span::new(10, 14),
        )
        .with_did_you_mean(Span::new(10, 14), "name");
        let dym = d.did_you_mean.expect("did-you-mean candidate present");
        assert_eq!(dym.wrong_span, Span::new(10, 14));
        assert_eq!(dym.replacement, "name");
    }
}
