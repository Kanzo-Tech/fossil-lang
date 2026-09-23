//! SC#5 verification — a revision bump / cancellation request mid-flight
//! unwinds an in-flight Salsa query via `Cancelled`, deterministically.
//!
//! ─────────────────────────────────────────────────────────────────────────
//! WHY THIS TEST EXISTS (and what SC#5's name reconciles to)
//! ─────────────────────────────────────────────────────────────────────────
//!
//! Phase 6 SC#5 (LSP-01) asks for "aggressive cancellation": a `didChange`
//! arriving mid-analysis must interrupt the in-flight work so the LSP never
//! ships a stale result. The success-criterion text named a `db.cancel_pending()`
//! method — that method does NOT exist in Salsa 0.26.2. The real cancellation
//! surface (verified against the vendored `salsa-0.26.2` source)
//! is revision-based and cooperative:
//!
//!   * `Database::cancellation_token() -> CancellationToken` + `token.cancel()`
//!     — sets the per-handle cancellation flag WITHOUT blocking. This is the
//!     deterministic trigger this test uses (it cannot deadlock against a live
//!     in-flight worker, unlike the Setter/`set_text` path which must block on
//!     `zalsa_mut` until snapshots reach a checkpoint).
//!   * `SourceFile::set_text(&mut db).to(..)` (a `salsa::Setter` mutation) OR
//!     `Database::synthetic_write(durability)` OR `Database::trigger_cancellation()`
//!     — the PRODUCTION trigger: each LSP `didChange` bumps the revision, which
//!     sets the same cancellation flag for other handles. Mechanism is identical
//!     to `token.cancel()` from the in-flight query's point of view: the next
//!     cooperative checkpoint observes the flag and unwinds. The unit-level
//!     proof uses `token.cancel()` because it cannot block; the LSP loop uses
//!     `set_text`, the revision bump that also carries the new text.
//!   * `Database::unwind_if_revision_cancelled(&self)` — the cooperative
//!     checkpoint a long query calls to bail early; it throws `Cancelled` (a
//!     panic-based unwind). Salsa ALSO inserts these checkpoints automatically
//!     at every query boundary (`EventKind::WillCheckCancellation`).
//!   * `salsa::Cancelled::catch(f)` — converts the cancellation unwind into
//!     `Err(Cancelled)` so the worker can discard the stale result cleanly.
//!
//! ─────────────────────────────────────────────────────────────────────────
//! DETERMINISM — no sleeps
//! ─────────────────────────────────────────────────────────────────────────
//!
//! The proof is fully deterministic via two `std::sync::Barrier`s (the exact
//! shape salsa's own `tests/cancellation_token.rs` uses). NO `thread::sleep`:
//!
//!   1. The worker thread runs the sentinel tracked query inside
//!      `Cancelled::catch`. The query records that it STARTED, then waits on
//!      `BARRIER_REACHED` (rendezvous 1: the query is provably in-flight).
//!   2. The main thread, after the same `BARRIER_REACHED` rendezvous, calls
//!      `token.cancel()` — sequencing the cancellation BEFORE the worker may
//!      proceed — then waits on `BARRIER_RELEASE`.
//!   3. The worker waits on `BARRIER_RELEASE` (rendezvous 2), then calls
//!      `db.unwind_if_revision_cancelled()`. Because the flag is now set, this
//!      UNWINDS with `Cancelled` BEFORE the query's post-checkpoint observable
//!      work runs.
//!   4. The test asserts `Cancelled::catch` returned `Err(Cancelled)` AND the
//!      `post_checkpoint_ran` flag is still `false` (the work after the
//!      checkpoint never executed — interruption is proven, not merely
//!      requested).
//!
//! A negative-control test (`uncancelled_query_runs_post_checkpoint_work`)
//! proves the sentinel is not vacuously passing: with NO cancellation, the
//! identical query runs to completion and DOES set `post_checkpoint_ran`.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, PoisonError};

use fossil_base::test_support::NativeSystem;
use fossil_base::{Db, FossilDb, SourceFile, System};
use salsa::{Cancelled, Database};

// ── Shared, thread-safe observation state for the sentinel tracked query.
//
// The sentinel CANNOT capture non-`'static` locals (Salsa tracked functions
// are free fns), so the coordination handles travel through process-global
// statics. Using statics keeps the sentinel a plain `#[salsa::tracked]` free
// function (no closure capture, no `Box<dyn>`), matching the constraint that
// Salsa interns concrete types.
//
// Both tests in this file share the test-binary process, so the two `#[test]`s
// must NOT run their barrier rendezvous concurrently (they would corrupt each
// other's `Barrier::new(2)` count). A process-global `RUN_LOCK` serialises the
// critical section; each test installs FRESH barriers under the lock via
// `reset_sentinel_state`, which swaps the re-settable `Mutex<Option<..>>` holders.

/// Serialises the two tests' rendezvous critical sections.
static RUN_LOCK: Mutex<()> = Mutex::new(());

/// Rendezvous 1: signalled by the sentinel once it is provably in-flight.
static BARRIER_REACHED: Mutex<Option<Arc<Barrier>>> = Mutex::new(None);
/// Rendezvous 2: released by the driver AFTER it has set the cancel flag.
static BARRIER_RELEASE: Mutex<Option<Arc<Barrier>>> = Mutex::new(None);
/// Set by the sentinel ONLY if it executes past the cancellation checkpoint.
static POST_CHECKPOINT_RAN: AtomicBool = AtomicBool::new(false);
/// Incremented when the sentinel body actually starts (proves in-flight).
static STARTED: AtomicUsize = AtomicUsize::new(0);

/// Install fresh barriers + clear observation state. Call under `RUN_LOCK`.
fn reset_sentinel_state(barrier_reached: &Arc<Barrier>, barrier_release: &Arc<Barrier>) {
    *BARRIER_REACHED.lock().unwrap() = Some(barrier_reached.clone());
    *BARRIER_RELEASE.lock().unwrap() = Some(barrier_release.clone());
    POST_CHECKPOINT_RAN.store(false, Ordering::SeqCst);
    STARTED.store(0, Ordering::SeqCst);
}

/// Clone the currently-installed rendezvous-1 barrier (the sentinel calls this).
fn installed_barrier_reached() -> Arc<Barrier> {
    BARRIER_REACHED
        .lock()
        .unwrap()
        .clone()
        .expect("BARRIER_REACHED installed")
}

/// Clone the currently-installed release barrier (the sentinel calls this).
fn installed_barrier_release() -> Arc<Barrier> {
    BARRIER_RELEASE
        .lock()
        .unwrap()
        .clone()
        .expect("BARRIER_RELEASE installed")
}

/// The sentinel slow query. A `#[salsa::tracked]` free function (no captured
/// state, no `Box<dyn>`) keyed on `SourceFile`. It rendezvouses with the
/// driver, then hits the cooperative cancellation checkpoint, and ONLY THEN
/// performs observable post-checkpoint work. If the revision was cancelled,
/// the checkpoint unwinds and the post-checkpoint work never runs.
#[salsa::tracked]
fn sentinel_slow_query(db: &dyn Db, file: SourceFile) -> usize {
    // Touch the input so the query genuinely depends on `file`'s revision —
    // this is what a real analysis query (parse/typecheck) would read.
    let len = file.text(db).len();

    STARTED.fetch_add(1, Ordering::SeqCst);

    // Rendezvous 1: prove we are in-flight before the driver cancels.
    installed_barrier_reached().wait();

    // Rendezvous 2: the driver has set the cancellation flag by the time this
    // returns (it sequences `token.cancel()` BEFORE releasing this barrier).
    installed_barrier_release().wait();

    // Cooperative cancellation checkpoint (the SC#5 surface). Salsa also
    // inserts these automatically at query boundaries, but a long-running
    // query calls it explicitly to bail promptly. If the flag is set, this
    // throws `Cancelled` and the rest of this function NEVER runs.
    db.unwind_if_revision_cancelled();

    // POST-CHECKPOINT OBSERVABLE WORK — reached ONLY when not cancelled.
    POST_CHECKPOINT_RAN.store(true, Ordering::SeqCst);
    len
}

/// SC#5 PROOF: a cancellation request mid-flight unwinds the in-flight query
/// via `Cancelled`, and the query's post-checkpoint work never runs.
#[test]
fn cancellation_mid_flight_unwinds_in_flight_query_via_cancelled() {
    let _run = RUN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let barrier_reached = Arc::new(Barrier::new(2));
    let barrier_release = Arc::new(Barrier::new(2));
    reset_sentinel_state(&barrier_reached, &barrier_release);

    // Count WillExecute for the sentinel so we can assert it actually ran.
    let will_execute: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let will_execute_cb = will_execute.clone();
    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if matches!(event.kind, salsa::EventKind::WillExecute { .. }) {
            will_execute_cb.fetch_add(1, Ordering::SeqCst);
        }
    });

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::with_event_callback(system, callback);
    let file = SourceFile::new(
        &db,
        "in-flight analysis target".to_string(),
        "c.fossil".to_string(),
    );

    // Worker: take a clone of the db handle (FossilDb is #[derive(Clone)]) and
    // run the sentinel inside Cancelled::catch — exactly the rust-analyzer / ty
    // model (analysis runs on a snapshot; the main handle stays live).
    //
    // Salsa's `CancellationToken` lives in the per-handle `ZalsaLocal`, which
    // `Storage::clone` re-creates fresh per clone (verified in vendored
    // `storage.rs`). So `token.cancel()` cancels queries on the SAME handle it
    // was taken from — we therefore acquire the token from the WORKER handle
    // (before moving it into the thread) and share the `Arc<AtomicU8>` token
    // with the main thread. (The production `set_text`/`synthetic_write` path
    // instead sets the SHARED runtime cancellation flag, which
    // `unwind_if_revision_cancelled` also checks.)
    let main_db = db; // the main handle; kept live for the worker's lifetime
    let worker_db = main_db.clone();
    let token = worker_db.cancellation_token();
    let worker = std::thread::spawn(move || {
        Cancelled::catch(AssertUnwindSafe(|| sentinel_slow_query(&worker_db, file)))
    });

    // Rendezvous 1: the sentinel is now provably in-flight (it has incremented
    // STARTED and is parked on BARRIER_REACHED).
    barrier_reached.wait();
    assert_eq!(
        STARTED.load(Ordering::SeqCst),
        1,
        "sentinel must be in-flight before we cancel"
    );

    // THE REAL CANCELLATION TRIGGER (non-blocking). In production the LSP loop
    // instead calls `file.set_text(&mut db).to(new)` on `didChange`, which
    // bumps the revision and sets this SAME flag; the mechanism the
    // in-flight query observes is identical.
    token.cancel();

    // Release the worker to its checkpoint — the flag is already set, so the
    // checkpoint unwinds.
    barrier_release.wait();

    let result = worker
        .join()
        .expect("worker thread did not panic-propagate");

    // The in-flight query unwound via Cancelled — NOT a normal return.
    assert!(
        matches!(result, Err(Cancelled::Local | Cancelled::PendingWrite)),
        "expected the in-flight query to unwind via Cancelled, got {result:?}"
    );

    // The load-bearing assertion: post-checkpoint work NEVER ran. Interruption
    // is PROVEN, not merely requested.
    assert!(
        !POST_CHECKPOINT_RAN.load(Ordering::SeqCst),
        "post-checkpoint work ran despite mid-flight cancellation — the \
         cooperative checkpoint did not interrupt the query (SC#5 violated)"
    );

    // Sanity: the sentinel really executed (it is not a no-op cache hit).
    assert!(
        will_execute.load(Ordering::SeqCst) >= 1,
        "sentinel query never emitted a WillExecute event"
    );

    // The main handle outlived the cancelled worker (rust-analyzer model): it
    // is still a usable Salsa database after a mid-flight cancellation.
    let _still_usable = main_db.cancellation_token();
}

/// NEGATIVE CONTROL: with NO cancellation, the identical sentinel query runs
/// to completion and DOES perform its post-checkpoint work. Proves the primary
/// test is not vacuously green (the checkpoint is not always-unwinding).
#[test]
fn uncancelled_query_runs_post_checkpoint_work() {
    let _run = RUN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let barrier_reached = Arc::new(Barrier::new(2));
    let barrier_release = Arc::new(Barrier::new(2));
    reset_sentinel_state(&barrier_reached, &barrier_release);

    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let main_db = FossilDb::new(system);
    let file = SourceFile::new(
        &main_db,
        "uncancelled target".to_string(),
        "c2.fossil".to_string(),
    );

    let worker_db = main_db.clone();
    let worker = std::thread::spawn(move || {
        Cancelled::catch(AssertUnwindSafe(|| sentinel_slow_query(&worker_db, file)))
    });

    // Rendezvous, but DO NOT cancel.
    barrier_reached.wait();
    barrier_release.wait();

    let result = worker
        .join()
        .expect("worker thread did not panic-propagate");
    let _still_usable = main_db.cancellation_token();

    assert!(
        result.is_ok(),
        "uncancelled query must complete normally, got {result:?}"
    );
    assert_eq!(result.unwrap(), "uncancelled target".len());
    assert!(
        POST_CHECKPOINT_RAN.load(Ordering::SeqCst),
        "uncancelled query must run its post-checkpoint work (negative control)"
    );
}
