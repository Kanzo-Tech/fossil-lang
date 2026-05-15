//! `ErrorGuaranteed` taint marker — borrowed from rustc.
//!
//! When a typeck query fails it returns `ErrorGuaranteed`. Downstream queries
//! that receive this taint short-circuit without emitting cascading errors.
//! N independent type errors produce N diagnostics, not N².
//!
//! `PhantomData<()>` (NOT `PhantomData<*const ()>`) preserves `Send + Sync`
//! so this taint can flow through the Salsa accumulator across threads.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorGuaranteed(std::marker::PhantomData<()>);

impl ErrorGuaranteed {
    /// Constructible only from inside `fossil-base` so callers cannot fabricate
    /// a taint without going through a diagnostic-emitting path. Phase 1 has
    /// no emitter yet; future plans extend this with `delay_span_bug`-style
    /// constructors after a real Diagnostic accumulation.
    #[must_use]
    #[allow(dead_code)] // emitted in HIR/typeck phases (1.3+); kept here as the substrate.
    pub(crate) const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}
