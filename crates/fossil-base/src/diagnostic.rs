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
//! manipulation.
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
//! `suggestion_source` precedent exactly — structured, not string-parsed.
//! Defaults to `None`; plain data (wasm-clean).

/// What a [`Diagnostic`]'s span was measured against.
///
/// A byte offset means nothing without knowing its origin, and fossil has two.
/// Per-mapping queries read `mapping_cst_node`, whose offsets rowan resets to
/// zero, so their spans are MAPPING-RELATIVE and a host must rebase them onto
/// the file before rendering (`fossil_hir::spans::rebase_to_file`). But some
/// diagnostics emitted from those same queries are about FILE-level syntax — a
/// `SOURCE_DEF`'s arguments, say — and rebasing those by the mapping's start
/// moves them somewhere meaningless.
///
/// Making the frame part of the diagnostic is what stops that from being an
/// unwritten rule nobody can check. `MappingRelative` is the default because
/// nearly every diagnostic comes from a per-mapping query.
// `Hash` because `SpanLabel` derives it and carries one. NOT MINE — one word,
// added because the whole workspace failed to compile without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SpanFrame {
    /// Offsets from the start of the enclosing mapping. Must be rebased.
    #[default]
    MappingRelative,
    /// Offsets from the start of the file. Already absolute; never rebase.
    FileAbsolute,
}

/// A SECOND place the same diagnostic points at, and what to say about it.
///
/// One diagnostic, several spans: `` `Users` and `Imported` mint two identities
/// for Person `` underlines both `@subject` lines and names each one, which is
/// the rule in as many words: the diagnostic NAMES the two mappings and the two
/// templates. A message that says «and one other mapping» makes the reader grep
/// for it.
///
/// # It carries its own [`SpanFrame`], and that is the whole point
///
/// A label routinely points at a DIFFERENT mapping than the one being blamed,
/// and per-mapping spans are mapping-relative
/// ([`fossil_hir::spans::rebase_to_file`](../../fossil_hir/spans/fn.rebase_to_file.html)).
/// So a label's offsets are not necessarily in the same frame as
/// [`Diagnostic::span`]: the emitter of a two-mapping report holds one span it
/// measured itself and one it took from elsewhere. Giving the label its own
/// frame is what stops that from being an unwritten rule — the rebasing layer
/// shifts each part by ITS OWN frame, so a file-absolute label survives inside
/// a mapping-relative diagnostic and vice versa.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpanLabel {
    /// The range to underline.
    pub span: Span,
    /// What to say about it, rendered under the caret.
    pub text: String,
    /// What [`Self::span`] was measured against — independently of the
    /// diagnostic's own [`Diagnostic::frame`].
    pub frame: SpanFrame,
}

impl SpanLabel {
    /// A label at `span`, measured against `frame`, saying `text`.
    #[must_use]
    pub fn new(span: Span, text: impl Into<String>, frame: SpanFrame) -> Self {
        Self {
            span,
            text: text.into(),
            frame,
        }
    }
}

#[salsa::accumulator]
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// What [`Self::span`] (and `did_you_mean.wrong_span`) were measured
    /// against — see [`SpanFrame`]. Hosts rebase only `MappingRelative` ones.
    pub frame: SpanFrame,
    /// The spans this one diagnostic points at — see [`SpanLabel`]. Empty for
    /// nearly every diagnostic; non-empty when the mistake is a RELATION
    /// between two places and naming one of them is not enough.
    ///
    /// **Rendering contract**: when this is empty a host underlines
    /// [`Self::span`] with a generic label, which is what every renderer did
    /// when a diagnostic had exactly one span. When it is NOT empty these
    /// replace that label entirely — a diagnostic that says what to underline
    /// says all of it, and one that also emitted `span` as an unnamed label
    /// would draw the same caret twice. [`Self::span`] stays authoritative for
    /// the single-position consumers: the editor's squiggle and any correlation
    /// by overlap.
    pub labels: Vec<SpanLabel>,
    /// The `help:` line rendered under the snippet: what to do about it, in
    /// prose.
    ///
    /// Distinct from [`Self::suggestion_source`], which is generated Fossil
    /// SOURCE a test compiles. Both reach miette's `#[help]`; this one wins,
    /// and no diagnostic sets both.
    pub help: Option<String>,
    /// Optional structured suggestion-source text. Populated by
    /// suggestion-emitting diagnostics (e.g. `ShEx` `OneOf` rejection's
    /// split-into-N-mappings code suggestion); read directly by consumers
    /// via the typed field, NOT via Markdown delimiter parsing. See module doc.
    pub suggestion_source: Option<String>,
    /// Optional structured did-you-mean candidate. Populated by a diagnostic
    /// whose [`Severity::Error`] message proposes a Levenshtein replacement for
    /// a typo'd identifier; read directly by the `fossil_ide::code_action`
    /// did-you-mean quick-fix to build a `WorkspaceEdit` replacing
    /// [`DidYouMean::wrong_span`] with [`DidYouMean::replacement`], NOT by
    /// parsing the message text. See module doc.
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
            frame: SpanFrame::MappingRelative,
            labels: Vec::new(),
            help: None,
            suggestion_source: None,
            did_you_mean: None,
        }
    }

    /// Point at a second place, and say what is there. See [`SpanLabel`] for
    /// why the frame is spelled out at every call site.
    #[must_use]
    pub fn with_label(mut self, span: Span, text: impl Into<String>, frame: SpanFrame) -> Self {
        self.labels.push(SpanLabel::new(span, text, frame));
        self
    }

    /// Attach the `help:` line — what to do about it, in prose.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Declare that this diagnostic's span is already file-absolute, so hosts
    /// leave it alone. For diagnostics about file-level syntax (a `SOURCE_DEF`,
    /// a prefix declaration) that happen to be emitted from a per-mapping query.
    #[must_use]
    pub const fn file_absolute(mut self) -> Self {
        self.frame = SpanFrame::FileAbsolute;
        self
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
            frame: SpanFrame::default(),
            labels: Vec::new(),
            help: None,
            suggestion_source: None,
            did_you_mean: None,
        };
        assert!(d.suggestion_source.is_none());
        assert!(d.did_you_mean.is_none());
        assert_eq!(
            d.frame,
            SpanFrame::MappingRelative,
            "the default frame is the one nearly every emitter is in"
        );
    }

    /// A diagnostic that blames two places carries the second as a label with
    /// its OWN frame — the file-absolute span of another mapping riding inside
    /// an otherwise mapping-relative report. Losing that distinction is what
    /// makes a caret land on a plausible, wrong line.
    #[test]
    fn a_label_keeps_its_own_frame() {
        let d = Diagnostic::new(Severity::Error, "two identities", Span::new(10, 20))
            .with_label(Span::new(400, 420), "`Users` mints this one", SpanFrame::FileAbsolute)
            .with_help("a type has one identity");
        assert_eq!(d.frame, SpanFrame::MappingRelative, "the default is unmoved");
        assert_eq!(d.labels.len(), 1);
        assert_eq!(d.labels[0].frame, SpanFrame::FileAbsolute);
        assert_eq!(d.help.as_deref(), Some("a type has one identity"));
    }

    #[test]
    fn diagnostic_default_did_you_mean_is_none() {
        let d = Diagnostic::new(Severity::Error, "oops", Span::new(0, 4));
        assert!(d.did_you_mean.is_none());
    }

    #[test]
    fn diagnostic_with_did_you_mean_carries_structured_candidate() {
        // The IDE quick-fix reads the (wrong_span, replacement) pair directly —
        // no message-string parsing.
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
