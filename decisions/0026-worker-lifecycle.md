# ADR 0026: Two Workers, two lifecycles (LSP long-lived; DuckDB terminate+recreate)

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Angel Iglesias (Fossil)
**Cite:** `.planning/phases/07-wasm-api-duckdb-wasm-base-playground/07-RESEARCH.md` §"Pattern 4" (Worker lifecycle) + §"Pattern 5" (dataset cap) + P-MOD-1 (DuckDB-WASM heap reclaim); ADR-0001 (fossil-lsp is native-only stdio); ADR-0024 (fossil-wasm = LSP server-side in the browser); Phase 7 plans 07-05 (LSP Worker), 07-06 (DuckDB-WASM Worker), 07-08 (Reset path + 10MB cap).

## Context

Phase 7 of the playground stands up TWO Web Workers, each backing a
distinct subsystem with a distinct lifecycle:

- **fossil-wasm LSP Worker** (07-05) — long-lived. The Worker is the
  LSP server-side per ADR-0024 (`fossil-lsp` cannot recompile to WASM
  because `lsp-server` uses crossbeam + stdio — neither WASM-portable;
  the `compile_error!` cfg-tripwire in `fossil-lsp` enforces this). The
  Worker holds the open Monaco editor models, the Salsa DB, the
  symbol indexes, and the user's edit history. Terminating it would
  destroy the editor session (models, cursor position, undo stack);
  the user would experience the Reset button as "lose all your typed
  code."
- **DuckDB-WASM Worker** (07-06) — short-lived. The Worker hosts a
  ~6.4MB WASM heap (the `mvp` bundle from `@duckdb/duckdb-wasm` 1.32.0;
  ADR-0025). Per P-MOD-1 (07-RESEARCH §"Pattern 4"), this heap is
  unreachable by JS GC: any CSV registered into it, any intermediate
  scratch table from a run, any temp file emitted by a `COPY ... TO`
  statement, all live until the Worker terminates. Across the SC#4
  scenario ("5 sequential Run/Reset cycles do not crash the tab"), the
  only reliable mechanism to reclaim that heap is
  `worker.terminate()` — hand-rolled `DROP TABLE` / `CHECKPOINT` /
  `pragma memory_limit` workarounds are forbidden by the research §"Don't
  Hand-Roll".

The forces in tension:

1. SC#4 requires that the playground survive 5 sequential Run/Reset
   cycles without memory blowup. That forces a `worker.terminate()`
   somewhere in the Reset path.
2. SC#2 + LSP-01 require that the editor session feel responsive across
   Resets (no perceived stutter, no lost edits, no LSP cold-start).
   That forbids `worker.terminate()` on the LSP side.
3. There is no shared state between the two Workers (the LSP Worker
   never reads the DuckDB heap; the DuckDB Worker never reads the LSP
   Salsa DB). Co-hosting them in a single Worker is structurally
   possible (the postMessage dispatcher could fan out by message tag)
   but would tie their lifecycles together, forcing the LSP to die on
   every example switch.

## Decision

**We will run two distinct Workers with two distinct lifecycles.** The
fossil-wasm LSP Worker is ONE per browser tab; it lives as long as the
editor session and is dropped only on tab close (or explicit
`LspChannel.dispose()` in tests). The DuckDB-WASM Worker is
**terminated and recreated** on every Reset button click — and (when
Phase 8 lands the example picker) on every example switch.

The TypeScript surface enforces this asymmetrically:

- `playground/src/duckdb/lifecycle.ts` exposes `resetDuckDb()`, which
  internally calls `__terminateWorker()` + `__clearForReset()` on the
  07-06 singletons.
- `playground/src/lifecycle/reset.ts` (07-08) calls **only**
  `resetDuckDb()`. There is no `resetLsp()` symbol anywhere in
  `playground/src`. Adding one would require a deliberate code change
  that this ADR is intended to make a reviewer pause over.

The Reset button's user-facing copy is "Reset playground" — never
"Reset Worker" — to keep the two-Worker reality out of the UI's
mental model.

## Consequences

**Positive.**

- SC#4 is mechanically dischargeable: the `worker.terminate()` in the
  Reset path is the documented reliable reclaim. 5 sequential
  Run/Reset cycles complete without memory growth (DevTools memory
  panel) and the SC#4 Playwright spec (07-10) can gate this in CI.
- The editor session stays warm across Resets: no LSP cold-start, no
  re-tokenize, no lost models. Reset feels instant to the user (the
  ~hundreds-of-ms cost is amortized into the next Run, where it
  hides under the existing progress indicator).
- The asymmetric API (`resetDuckDb` exists, `resetLsp` does not)
  makes the decision auditable in code: a future contributor cannot
  accidentally widen Reset to nuke the LSP Worker.

**Negative.**

- The first Run after every Reset re-pays the ~6.4MB DuckDB-WASM
  instantiation cost. Browser cache typically keeps the payload
  around so the network cost is zero, but the WASM compile +
  instantiate is real (~hundreds of ms on a 2020-era MacBook; longer
  on lower-end devices). SC#2's <5s Run budget absorbs this with
  comfortable headroom for the canonical example.
- The playground UI must NEVER refer to "the Worker" in copy — there
  are two, and which one a given message refers to is non-obvious.
  Code review must flag any string that says "Worker" without
  qualification.
- The codebase now has two distinct Worker-lifecycle conventions to
  remember. Mitigated by this ADR + header comments in
  `lifecycle.ts` + `reset.ts` that cite it.

**Neutral.**

- The 10MB CSV cap (07-08 / SC#4 / PLAY-12) is a separate but
  related defence: even with `terminate()`, a 50MB CSV would blow
  the heap mid-run BEFORE Reset could fire. The cap and the
  lifecycle decision together discharge SC#4. The cap lives on the
  main thread (UI refusal BEFORE bytes reach either Worker); the
  lifecycle decision lives in the Worker-supervision code.

## Alternatives considered

- **Single Worker hosting both subsystems (LSP + DuckDB).** Rejected.
  Co-locating them would couple the lifecycles — every Reset would
  destroy the editor session. The structural simplicity gain (one
  postMessage dispatch loop) does not outweigh the UX cost. Also
  defeats ADR-0024's "one crate, two hosts" vision (the LSP Worker IS
  fossil-wasm; folding DuckDB into the same Worker would require
  either a second-class JS sidecar inside the Worker or bundling
  duckdb-wasm into the Rust binary).
- **On-demand DuckDB Worker per Run.** Rejected. A 6.4MB cold-start
  on every Run would blow the SC#2 <5s budget on slower devices and
  remove the cache-warm path entirely. Reset-bounded recreate is the
  middle ground: warm within a session, cold across Resets.
- **Worker recycling pool (pre-warmed standby Worker swapped in on
  Reset).** Rejected as premature optimization. The playground is a
  one-tab solo-user deployment; there is no contention or latency
  pressure that a pool would relieve. The terminate+recreate cost is
  comfortably under SC#2 budget without it.
- **Hand-rolled heap reclaim (DROP TABLE / CHECKPOINT / pragmas
  between runs).** Rejected by research §"Don't Hand-Roll":
  DuckDB-WASM's `mvp` bundle does not surface a public, supported
  path to forcibly release the WASM linear memory back to the OS; the
  available DROPs reclaim catalog space but not heap. `terminate()`
  is the supported mechanism.
