# ADR 0022: Use Salsa's revision-based cooperative cancellation; verify it with a deterministic Barrier test

**Date:** 2026-05-22
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:** `.planning/phases/06-cli-complete-lsp/06-RESEARCH.md` §"Salsa Cancellation in the LSP loop (SC#5)" + §Pitfall 2; vendored `salsa-0.26.2/src/{database,cancelled,zalsa,zalsa_local,storage}.rs`; ADR-0001 (lsp-server sync model)

## Context

Phase 6 success-criterion SC#5 (requirement LSP-01) calls for "aggressive
cancellation": when a `didChange` notification arrives while the language
server is still analysing a previous edit, the in-flight analysis must be
interrupted so the server never publishes a stale result. The roadmap text
named a method `db.cancel_pending()`.

That method does **not exist** in Salsa 0.26.2. Reading the vendored source
establishes the actual cancellation surface, which is revision-based and
cooperative rather than an explicit "cancel the pending query" call:

- `Database::cancellation_token() -> CancellationToken` returns a per-handle
  token; `token.cancel()` sets that handle's local cancellation flag WITHOUT
  blocking. The token lives in `ZalsaLocal`, and `Storage::clone` constructs a
  fresh `ZalsaLocal` per clone — so a token cancels queries running on the
  handle it was taken from.
- A write bumps the revision and sets the SHARED runtime cancellation flag:
  a `salsa::Setter` mutation (`SourceFile::set_text(&mut db).to(new)`),
  `Database::synthetic_write(durability)`, or `Database::trigger_cancellation()`.
  These take `&mut db`, which calls `zalsa_mut()` → `cancel_others()`, which
  sets the flag and then BLOCKS until all other handles reach a checkpoint or
  drop (so it must not be called while the current thread holds a live snapshot
  of the same db — that would deadlock).
- `Database::unwind_if_revision_cancelled(&self)` is the cooperative checkpoint
  a long-running query calls to bail promptly; it checks both the local token
  flag and the shared runtime flag and, if either is set, unwinds via a
  `Cancelled` panic. Salsa ALSO inserts this checkpoint automatically at every
  query boundary (emitting `EventKind::WillCheckCancellation`).
- `salsa::Cancelled::catch(f)` converts the cancellation unwind into
  `Err(Cancelled)` so the caller can discard the stale result cleanly.
  `Cancelled` is `{ Local, PendingWrite, PropagatedPanic }`.

ADR-0001 fixed the LSP transport to the synchronous `lsp-server` + crossbeam
model. In a purely synchronous loop there is nothing in-flight to cancel — the
edit is applied inline and Salsa recomputes lazily on the next query. So SC#5's
"interrupt in-flight work" is only observable once analysis runs on a worker
that holds a db snapshot while the main thread bumps the revision. Whether the
production loop ships as a threaded worker or as lazy-recompute is a separate
decision finalised in the LSP plan (06-08); SC#5 must be verifiable regardless
of that choice.

## Decision

We will use Salsa's native revision-based cooperative cancellation as the
cancellation mechanism — there is no custom `AtomicBool` flag and no bespoke
"cancel pending" call. In production the LSP loop applies each `didChange` via
a `Setter` mutation (`SourceFile::set_text(&mut db)`), which bumps the revision
and sets the shared cancellation flag; any analysis running on a snapshot
handle, wrapped in `Cancelled::catch`, unwinds at its next
`unwind_if_revision_cancelled` checkpoint (automatic at query boundaries, or
explicit in long queries). The SC's aspirational `db.cancel_pending()` name
reconciles to this real surface.

We will verify SC#5 at the Salsa unit level with a deterministic test
(`crates/fossil-hir/tests/cancellation.rs`) that is independent of the eventual
production loop model. A sentinel `#[salsa::tracked]` query runs on a cloned db
handle inside `Cancelled::catch`; two `std::sync::Barrier`s (no `thread::sleep`)
sequence a `token.cancel()` (taken from the worker handle, since the token is
per-handle) strictly before the worker reaches its
`unwind_if_revision_cancelled` checkpoint. The test asserts the query unwinds
via `Cancelled` AND that observable work placed AFTER the checkpoint never ran —
proving interruption rather than merely requesting it. A negative-control test
proves the sentinel is not vacuously green.

The unit test uses `token.cancel()` rather than `set_text` as the trigger
because `set_text`/`synthetic_write`/`trigger_cancellation` require `&mut db`
and block on live snapshots, which would deadlock against a worker that is still
parked at a barrier. From the in-flight query's point of view the two triggers
are identical — both set a flag that `unwind_if_revision_cancelled` observes —
so the unit-level proof faithfully exercises the production mechanism.

## Consequences

- **Cancellation is free and correct.** It rides Salsa's existing revision
  machinery; there is no parallel cancellation state to keep consistent and no
  checkpoint code to sprinkle through queries (Salsa inserts them at query
  boundaries automatically).
- **SC#5 is verified at the unit level, decoupled from the loop model.** The
  06-08 LSP plan can ship the simplest model that keeps the perf benchmark green
  (threaded worker OR lazy-recompute, Research Open Question #4) without
  re-litigating cancellation correctness — the proof already stands.
- **The "no sleeps" determinism rule is honoured**, so the test is CI-stable
  (no timing flake; mirrors salsa's own `tests/cancellation_token.rs` shape).
- **A subtlety becomes load-bearing knowledge:** the `CancellationToken` is
  per-handle (per `ZalsaLocal`), so a token must be taken from the same handle
  the cancelled query runs on; the shared runtime flag (set by a revision bump)
  is the cross-handle path. This is documented in the test and here so a future
  reader does not wire a main-handle token to a worker-handle query and see a
  silently non-cancelling no-op.
- **Negative risk:** because `&mut db` mutations block on live snapshots, a
  future threaded LSP worker must drop or checkpoint its snapshot promptly; a
  worker that never reaches a checkpoint would stall the main thread's
  `set_text`. The cooperative-checkpoint design (automatic at query boundaries)
  makes this a non-issue for normal query graphs, but a hand-written hot loop
  inside a single query should call `unwind_if_revision_cancelled` periodically.

### Alternatives considered

- **Custom `AtomicBool` cancellation flag checked throughout the checker.**
  Rejected: duplicates and fights Salsa's revision machinery (Research §"Don't
  Hand-Roll"), needs manual checkpoint placement, and risks inconsistency with
  Salsa's own memoization invalidation.
- **Use `set_text`/`synthetic_write` as the unit-test trigger.** Rejected for
  the unit test only: it blocks on the live worker snapshot and would deadlock
  against the barrier rendezvous. It remains the correct PRODUCTION trigger
  (it carries the new revision's text), and the test documents the equivalence.
