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
//! `ErrorGuaranteed` cannot be constructed outside this module. The two
//! public construction paths are [`delay_span_bug`] and [`bug`], both of
//! which push at least one [`Diagnostic`] to the Salsa accumulator before
//! returning the taint. There is no public path from external code to
//! `ErrorGuaranteed::new()`. The invariant is structurally enforced — a
//! `Default` impl would defeat it and is intentionally omitted.

use crate::diagnostic::{Diagnostic, Severity, Span};
use salsa::Accumulator;

/// Type-check failure taint.
///
/// Construct via [`delay_span_bug`] or [`bug`] — both push a [`Diagnostic`]
/// to the accumulator before returning, so any `ErrorGuaranteed` value
/// implies "at least one diagnostic was emitted".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorGuaranteed(std::marker::PhantomData<()>);

impl ErrorGuaranteed {
    /// Module-private constructor — only [`delay_span_bug`] / [`bug`] call
    /// this. Keeping it private is what enforces the "at least one diagnostic
    /// per `ErrorGuaranteed`" construction invariant.
    #[must_use]
    const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

// SAFETY: a third-party-trait integration boundary, which is the ONLY thing the
// workspace's `unsafe_code = "deny"` (not `"forbid"`) exists to let through, and
// only with a justification naming what the unsafe is for. ErrorGuaranteed
// is Copy + Eq + Hash with no nested invariants (PhantomData<()> is zero-sized),
// so the trivial-replace pattern is sound. No safe alternative exists because
// salsa::Update requires `unsafe impl` even for trivially-safe bodies.
#[allow(unsafe_code)]
unsafe impl salsa::Update for ErrorGuaranteed {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: caller (Salsa) guarantees `old_pointer` is a valid, aligned
        // pointer to an initialised `ErrorGuaranteed` owned by Salsa storage.
        let old = unsafe { &mut *old_pointer };
        if *old == new_value {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

/// Emit a deferred bug diagnostic and return the taint.
///
/// Use this when a type-checker invariant is violated but recovery is still
/// possible (the caller can continue with a poisoned `Ty::Error`). Mirrors
/// rustc's `tcx.dcx().delayed_bug(...)` API.
///
/// **Invariant:** every call to `delay_span_bug` pushes EXACTLY ONE
/// [`Diagnostic`] to the accumulator and returns a fresh [`ErrorGuaranteed`].
/// The Salsa accumulator's per-query collection semantics ensure the
/// diagnostic flows to the LSP / CLI host via
/// `query::accumulated::<Diagnostic>(db, ...)`.
#[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
pub fn delay_span_bug(
    db: &dyn crate::Db,
    span: Span,
    message: impl Into<String>,
) -> ErrorGuaranteed {
    // `delay_span_bug` is the generic taint emitter — it carries neither a
    // structured suggestion source nor a did-you-mean candidate.
    // Suggestion-emitting call sites build their own `Diagnostic` and attach
    // the structured fields via the builders.
    raise(db, Diagnostic::new(Severity::Error, message, span))
}

/// Emit a diagnostic the caller has already BUILT, and return the taint.
///
/// The one taint maker; [`delay_span_bug`] is this with the diagnostic built
/// for you. It exists because a caller that has something to say about the
/// diagnostic — a `SpanFrame`, a did-you-mean, a suggestion source — had to
/// choose between the builders and the taint, and the two are not alternatives:
/// `ErrorGuaranteed::new` is private, so building your own diagnostic meant
/// emitting it and then calling `delay_span_bug` for the taint, which pushes a
/// SECOND diagnostic. The invariant below is what that would have broken.
///
/// **Invariant:** exactly one [`Diagnostic`] per call, and a fresh
/// [`ErrorGuaranteed`].
#[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
pub fn raise(db: &dyn crate::Db, diagnostic: Diagnostic) -> ErrorGuaranteed {
    diagnostic.accumulate(db);
    ErrorGuaranteed::new()
}

/// Same as [`delay_span_bug`] but for compiler-internal bugs.
///
/// Severity stays `Error` but the message is prefixed
/// `"internal compiler error: "`. For "this code path should be unreachable"
/// cases, where the user has done nothing wrong and the compiler has.
#[must_use = "ErrorGuaranteed must be propagated to the caller to taint downstream queries"]
pub fn bug(db: &dyn crate::Db, span: Span, message: impl Into<String>) -> ErrorGuaranteed {
    delay_span_bug(
        db,
        span,
        format!("internal compiler error: {}", message.into()),
    )
}
