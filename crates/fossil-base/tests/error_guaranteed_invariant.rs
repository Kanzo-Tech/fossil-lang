//! Contract test: [`fossil_base::ErrorGuaranteed`] invariants.
//!
//! Per RESEARCH.md §Q6 + plan 02-05 Task 2:
//!
//! 1. Every construction path ([`delay_span_bug`] / [`bug`]) returns an
//!    `ErrorGuaranteed` AND accumulates at least one [`Diagnostic`].
//! 2. `ErrorGuaranteed` cannot be `Default`-constructed (negative-compile
//!    assertion documented as a comment — promoted to a `trybuild` test
//!    in Phase 3 when we have enough emission sites to make a corpus).
//! 3. `ErrorGuaranteed` is `Send + Sync` (`PhantomData<()>` preserves both;
//!    `PhantomData<*const ()>` would not — guarding against accidental
//!    regression of the marker type).
//! 4. The full accumulator round-trip works inside a `#[salsa::tracked]`
//!    query — proving end-to-end "any tracked query that calls
//!    `delay_span_bug` exposes the diagnostic to the host via
//!    `Diagnostic::accumulated`".

use fossil_base::{
    Db, Diagnostic, FossilDb, NativeSystem, Severity, SourceFile, Span, System, bug, delay_span_bug,
};
use std::sync::Arc;

fn db_with_file() -> (FossilDb, SourceFile) {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, "x".to_string(), "x.fossil".to_string());
    (db, file)
}

/// A tracked-query shim that calls `delay_span_bug` from inside Salsa
/// context. Accumulator state collects per-query; this is the only context
/// in which `Diagnostic::accumulated(db, query_input)` returns the pushed
/// diagnostic.
#[salsa::tracked]
fn tracked_emit_delay(db: &dyn Db, file: SourceFile, msg_offset: u32) -> u32 {
    let _ = file.text(db); // touch the input so the query has a real dependency
    let _ = delay_span_bug(db, Span::new(msg_offset, msg_offset + 1), "test error");
    msg_offset
}

/// A tracked-query shim that calls [`bug`] (which delegates to
/// [`delay_span_bug`] but prefixes the message). Used to verify the
/// "internal compiler error: " prefix path also reaches the accumulator.
#[salsa::tracked]
fn tracked_emit_bug(db: &dyn Db, file: SourceFile) -> u32 {
    let _ = file.text(db);
    let _ = bug(db, Span::new(0, 1), "unreachable code reached");
    0
}

#[test]
fn delay_span_bug_emits_one_diagnostic_via_accumulator() {
    let (db, file) = db_with_file();
    let _ = tracked_emit_delay(&db, file, 7);
    let diags = tracked_emit_delay::accumulated::<Diagnostic>(&db, file, 7);
    assert_eq!(
        diags.len(),
        1,
        "delay_span_bug must push exactly one Diagnostic per call"
    );
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(diags[0].message, "test error");
    assert_eq!(diags[0].span, Span::new(7, 8));
}

#[test]
fn bug_prefixes_internal_compiler_error_in_accumulated_diagnostic() {
    let (db, file) = db_with_file();
    let _ = tracked_emit_bug(&db, file);
    let diags = tracked_emit_bug::accumulated::<Diagnostic>(&db, file);
    assert_eq!(diags.len(), 1, "bug must push exactly one Diagnostic");
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(
        diags[0].message,
        "internal compiler error: unreachable code reached"
    );
}

/// A tracked-query shim that captures the `ErrorGuaranteed` return so we
/// can pin its type. Salsa requires accumulator pushes to happen inside a
/// tracked context — calling `delay_span_bug` from outside one panics
/// ("cannot accumulate values outside of an active tracked function"),
/// which is exactly the upstream behaviour we want (it forces the
/// invariant "`ErrorGuaranteed` is only constructed via a tracked
/// diagnostic-emitting path").
#[salsa::tracked]
fn tracked_returns_eg(db: &dyn Db, file: SourceFile) -> u32 {
    let _ = file.text(db);
    let eg: fossil_base::ErrorGuaranteed = delay_span_bug(db, Span::new(0, 1), "x");
    // The variable assignment pins the return type — if `delay_span_bug`'s
    // signature changes from `-> ErrorGuaranteed` this stops compiling.
    let _ = eg;
    0
}

#[test]
fn delay_span_bug_returns_error_guaranteed_value() {
    let (db, file) = db_with_file();
    let _ = tracked_returns_eg(&db, file);
    let diags = tracked_returns_eg::accumulated::<Diagnostic>(&db, file);
    assert_eq!(diags.len(), 1, "delay_span_bug must push one Diagnostic");
}

#[test]
fn error_guaranteed_cannot_be_default_constructed() {
    // This test PASSES by compiling. The negative assertion is at the type
    // level: uncommenting the next line MUST fail compilation with E0277
    // ("the trait `Default` is not implemented for `ErrorGuaranteed`").
    //
    //   let _: fossil_base::ErrorGuaranteed = Default::default();
    //
    // Trybuild-style negative compile tests are deferred to Phase 3 when we
    // have enough error-emission sites to make a meaningful corpus.
    fn _no_default<T: Default>() {}
    // _no_default::<fossil_base::ErrorGuaranteed>();   // would not compile
}

#[test]
fn error_guaranteed_is_send_and_sync() {
    // PhantomData<()> preserves Send + Sync per RESEARCH.md §Q6. Verify at
    // the type level so a future change to PhantomData<*const ()> (which
    // would break the Salsa accumulator cross-thread flow) trips compilation
    // here.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<fossil_base::ErrorGuaranteed>();
}
