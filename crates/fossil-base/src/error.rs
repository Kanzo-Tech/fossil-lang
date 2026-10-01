//! `ErrorGuaranteed` taint marker — borrowed from rustc.
//!
//! When a typeck query fails it emits a [`Diagnostic`] AND returns
//! [`ErrorGuaranteed`]. Downstream queries that receive this taint short-
//! circuit without emitting cascading errors. N independent type errors
//! produce N diagnostics, not N².
//!
//! `PhantomData<()>` (NOT `PhantomData<*const ()>`) preserves `Send + Sync`
//! so this taint can flow through the Salsa accumulator across threads: the
//! raw-pointer variant would un-impl `Send`/`Sync` and break the `Diagnostic`
//! accumulator's cross-thread flow.
//!
//! # Construction invariant
//!
//! `ErrorGuaranteed` cannot be constructed outside this module. Every public
//! path in — [`raise`], and [`report`] / [`bug`] which go through it — pushes
//! exactly one [`Diagnostic`] to the Salsa accumulator before returning the
//! taint. There is no public path from external code to
//! `ErrorGuaranteed::new()`. The invariant is structurally enforced — a
//! `Default` impl would defeat it and is intentionally omitted.

use crate::diagnostic::{Diagnostic, Problem, Severity, Span};
use salsa::Accumulator;

/// Type-check failure taint.
///
/// Construct via [`raise`], [`report`] or [`bug`] — each pushes a
/// [`Diagnostic`] to the accumulator before returning, so any
/// `ErrorGuaranteed` value implies "at least one diagnostic was emitted".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct ErrorGuaranteed(std::marker::PhantomData<()>);

impl ErrorGuaranteed {
    /// Module-private constructor — [`raise`] is its one caller, and every
    /// other path in reaches it through there. Keeping it private is what
    /// enforces the "one diagnostic per `ErrorGuaranteed`" invariant.
    #[must_use]
    const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

/// Emit a diagnostic the caller has already BUILT, and return the taint.
///
/// The one taint maker; [`report`] and [`bug`] are this with the diagnostic
/// built for you. A caller with something to say about the diagnostic — a
/// `SpanFrame`, a label, a did-you-mean — builds it and comes here, because
/// building one, emitting it and then asking a builder for the taint would push
/// a SECOND diagnostic. The invariant below is what that would break.
///
/// **Invariant:** exactly one [`Diagnostic`] per call, and a fresh
/// [`ErrorGuaranteed`].
#[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
pub fn raise(db: &dyn crate::Db, diagnostic: Diagnostic) -> ErrorGuaranteed {
    diagnostic.accumulate(db);
    ErrorGuaranteed::new()
}

/// Report `problem` at `span` as an error — mapping-relative, no label, no
/// help — and return the taint.
#[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
pub fn report(db: &dyn crate::Db, span: Span, problem: Problem) -> ErrorGuaranteed {
    raise(db, Diagnostic::new(Severity::Error, problem, span))
}

/// Report fossil's own fault: a path the program cannot have reached, where the
/// author has done nothing wrong and the compiler has. [`Problem::Bug`].
#[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
pub fn bug(db: &dyn crate::Db, span: Span, what: impl Into<String>) -> ErrorGuaranteed {
    report(db, span, Problem::Bug { what: what.into() })
}
