//! `ErrorGuaranteed` taint marker — borrowed from rustc — and the
//! [`Diagnostics`] sink that is the only thing able to mint one.
//!
//! When a check fails it pushes a [`Diagnostic`] into the sink it was handed
//! AND returns [`ErrorGuaranteed`]. Downstream queries that receive this taint
//! short-circuit without emitting cascading errors. N independent type errors
//! produce N diagnostics, not N².
//!
//! `PhantomData<()>` (NOT `PhantomData<*const ()>`) preserves `Send + Sync`:
//! the taint is stored in query outputs, which Salsa shares across threads.
//!
//! # Construction invariant
//!
//! `ErrorGuaranteed` cannot be constructed outside this module. Every public
//! path in — [`Diagnostics::raise`], and [`Diagnostics::report`] /
//! [`Diagnostics::bug`] which go through it — pushes exactly one
//! [`Diagnostic`] into the sink before returning the taint. There is no public
//! path from external code to `ErrorGuaranteed::new()`. The invariant is
//! structurally enforced — a `Default` impl would defeat it and is
//! intentionally omitted.

use crate::diagnostic::Diagnostic;
use fossil_graph_schema::{Problem, Severity, Span};

/// Type-check failure taint.
///
/// Construct via [`Diagnostics::raise`], [`Diagnostics::report`] or
/// [`Diagnostics::bug`] — each pushes a [`Diagnostic`] into the sink before
/// returning, so any `ErrorGuaranteed` value implies "at least one diagnostic
/// was emitted".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct ErrorGuaranteed(std::marker::PhantomData<()>);

impl ErrorGuaranteed {
    /// Module-private constructor — [`Diagnostics::raise`] is its one caller,
    /// and every other path in reaches it through there. Keeping it private is
    /// what enforces the "one diagnostic per `ErrorGuaranteed`" invariant.
    #[must_use]
    const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

/// Where a check puts what it found.
///
/// A query that can find a mistake owns one, hands it to its helpers by
/// `&mut`, and returns its contents in its output — so a helper needs no
/// database to report into, and a test hands it a sink of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    /// An empty sink.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Emit a diagnostic the caller has already BUILT, and return the taint.
    ///
    /// The one taint maker; [`Self::report`] and [`Self::bug`] are this with
    /// the diagnostic built for you. A caller with something to say about the
    /// diagnostic — a `SpanFrame`, a label, a did-you-mean — builds it and comes
    /// here, because building one, emitting it and then asking a builder for
    /// the taint would push a SECOND diagnostic. The invariant above is what
    /// that would break.
    ///
    /// **Invariant:** exactly one [`Diagnostic`] per call, and a fresh
    /// [`ErrorGuaranteed`].
    #[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
    pub fn raise(&mut self, diagnostic: Diagnostic) -> ErrorGuaranteed {
        self.0.push(diagnostic);
        ErrorGuaranteed::new()
    }

    /// Report `problem` at `span` as an error — mapping-relative, no label, no
    /// help — and return the taint.
    #[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
    pub fn report(&mut self, span: Span, problem: Problem) -> ErrorGuaranteed {
        self.raise(Diagnostic::new(Severity::Error, problem, span))
    }

    /// Report fossil's own fault: a path the program cannot have reached, where
    /// the author has done nothing wrong and the compiler has. [`Problem::Bug`].
    #[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
    pub fn bug(&mut self, span: Span, what: impl Into<String>) -> ErrorGuaranteed {
        self.report(span, Problem::Bug { what: what.into() })
    }

    /// Emit a diagnostic that poisons nothing: a warning, or an error whose
    /// query still returns a usable answer.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.0.push(diagnostic);
    }

    /// What was emitted, in emission order.
    #[must_use]
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.0
    }

    /// The emitted diagnostics, for a query's output.
    #[must_use]
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.0
    }
}

impl Extend<Diagnostic> for Diagnostics {
    fn extend<I: IntoIterator<Item = Diagnostic>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}
