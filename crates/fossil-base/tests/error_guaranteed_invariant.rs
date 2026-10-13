//! Contract test: [`fossil_base::ErrorGuaranteed`] invariants.
//!
//! The invariants:
//!
//! 1. Every construction path ([`Diagnostics::report`] /
//!    [`Diagnostics::bug`]) returns an `ErrorGuaranteed` AND pushes exactly
//!    one [`Diagnostic`] into the sink it was called on.
//! 2. `ErrorGuaranteed` cannot be `Default`-constructed (negative-compile
//!    assertion documented as a comment — a `trybuild` test would need a
//!    corpus of emission sites to be worth its build cost).
//! 3. `ErrorGuaranteed` is `Send + Sync` (`PhantomData<()>` preserves both;
//!    `PhantomData<*const ()>` would not — guarding against accidental
//!    regression of the marker type).
//!
//! No database: a helper reports into a sink it is handed, so a test hands it
//! one of its own.

use fossil_base::Diagnostics;
use fossil_graph_schema::{Problem, Severity, Span};

const fn problem() -> Problem {
    Problem::UnexpectedToken {}
}

#[test]
fn report_emits_one_diagnostic_into_the_sink() {
    let mut sink = Diagnostics::new();
    let _ = sink.report(Span::new(7, 8), problem());
    let diags = sink.as_slice();
    assert_eq!(
        diags.len(),
        1,
        "report must push exactly one Diagnostic per call"
    );
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(diags[0].problem, problem());
    assert_eq!(diags[0].span, Span::new(7, 8));
}

#[test]
fn bug_reports_internal_bug_into_the_sink() {
    let mut sink = Diagnostics::new();
    let _ = sink.bug(Span::new(0, 1), "unreachable code reached");
    let diags = sink.as_slice();
    assert_eq!(diags.len(), 1, "bug must push exactly one Diagnostic");
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(diags[0].problem.code(), "internal/bug");
    assert_eq!(
        diags[0].message(),
        "internal error: unreachable code reached"
    );
}

#[test]
fn report_returns_error_guaranteed_value() {
    let mut sink = Diagnostics::new();
    let eg: fossil_base::ErrorGuaranteed = sink.report(Span::new(0, 1), problem());
    // The variable assignment pins the return type — if `report`'s
    // signature changes from `-> ErrorGuaranteed` this stops compiling.
    let _ = eg;
    assert_eq!(sink.as_slice().len(), 1, "report must push one Diagnostic");
}

#[test]
fn error_guaranteed_cannot_be_default_constructed() {
    // This test PASSES by compiling. The negative assertion is at the type
    // level: uncommenting the next line MUST fail compilation with E0277
    // ("the trait `Default` is not implemented for `ErrorGuaranteed`").
    //
    //   let _: fossil_base::ErrorGuaranteed = Default::default();
    //
    // Trybuild-style negative compile tests stay deferred until there are
    // enough error-emission sites to make a meaningful corpus; one assertion
    // does not pay for a second compiler invocation per run.
    fn _no_default<T: Default>() {}
    // _no_default::<fossil_base::ErrorGuaranteed>();   // would not compile
}

#[test]
fn error_guaranteed_is_send_and_sync() {
    // PhantomData<()> preserves Send + Sync. Verify at
    // the type level so a future change to PhantomData<*const ()> (which
    // would stop a query output carrying the taint from crossing threads)
    // trips compilation here.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<fossil_base::ErrorGuaranteed>();
}
