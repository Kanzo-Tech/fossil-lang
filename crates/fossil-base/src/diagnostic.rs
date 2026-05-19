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
}

impl Diagnostic {
    /// Build a [`Diagnostic`] with no attached suggestion source.
    ///
    /// Equivalent to the struct literal with `suggestion_source: None`. Use
    /// [`Self::with_suggestion_source`] to attach one fluently.
    #[must_use]
    pub fn new(severity: Severity, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity,
            message: message.into(),
            span,
            suggestion_source: None,
        }
    }

    /// Attach a suggestion-source string (e.g. the generated split snippet
    /// for `ShEx` `OneOf` rejection).
    #[must_use]
    pub fn with_suggestion_source(mut self, source: impl Into<String>) -> Self {
        self.suggestion_source = Some(source.into());
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
        };
        assert!(d.suggestion_source.is_none());
    }
}
