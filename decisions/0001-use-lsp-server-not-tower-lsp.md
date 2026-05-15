# ADR 0001: Use lsp-server, not tower-lsp

**Date:** 2026-05-15
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/research/STACK.md` §LSP layer; `.planning/research/ARCHITECTURE.md` §Pattern 2 — LSP framework

## Context

Fossil's `architecture.md` design corpus (committed before research synthesis) named `tower-lsp` as the LSP framework. Subsequent research surfaced two material findings against that choice:

- `tower-lsp` upstream has been **unmaintained for ~3 years** (last release pre-dates 2023, no merged PRs in 18+ months as of May 2026). Multiple known issues with notification ordering and worker shutdown remain open. The `async-lsp` fork (oxalica) is one alternative; `tower-lsp-server` (community fork, active April 2026) is another.
- All three reference codebases the project explicitly cites — **rust-analyzer, ty (Astral, formerly red-knot), and gleam** — use **`lsp-server`** (the rust-analyzer-team-maintained crate), not `tower-lsp` and not its forks. The synchronous crossbeam-channel dispatch model maps cleanly to Salsa's "set input → in-flight queries panic with `Cancelled` → workers catch the panic" cancellation pattern, which is how rust-analyzer achieves sub-100ms LSP response.

The choice is not between async vs sync LSP frameworks; it is between aligning with the reference codebases (`lsp-server`, sync) versus introducing a different concurrency model (any of the tower-lsp-family crates, async). Aligning means the patterns and idioms documented in those repos transfer directly to Fossil.

## Decision

We will use **`lsp-server 0.7`** (with `lsp-types 0.97` and `crossbeam-channel 0.5` as direct deps) for the `fossil-lsp` crate.

`tower-lsp`, `tower-lsp-server`, and `async-lsp` are explicitly out of scope for v0.1. They are added to `deny.toml`'s ban list (alongside `serde_yml`, `wasm-pack`, `sqlx`) so a future contributor cannot reintroduce them without an explicit follow-up ADR.

## Consequences

**Positive:**
- Direct alignment with rust-analyzer / ty / gleam — patterns transfer, search results from those repos apply, contributors with rust-analyzer experience are immediately productive.
- No `tokio` runtime in the LSP path, which keeps the WASM CI gate clean (the `fossil-lsp` crate is native-only by design, but transitive `tokio` would make the workspace check noisier).
- Salsa cancellation maps to the sync crossbeam pattern with one shared `CancellationToken` checked in worker hot loops — well-trodden.
- Minimal "magic" — the dispatch loop is ~200 LOC of explicit message routing, easy to debug.

**Negative:**
- ~200 LOC of dispatch boilerplate in `fossil-lsp` versus ~50 LOC of `#[tower_lsp::async_trait]` decoration. Acceptable cost given the alignment benefit.
- No async ergonomics inside the LSP loop. If Fossil ever needs to push diagnostics from a non-Salsa thread, a manual channel + dispatch hop is required. Phase 1-7 do not need this.

**Neutral:**
- The VS Code extension (Phase 9) is invariant under the LSP framework choice — the extension talks JSON-RPC, the framework on the server side is invisible.
- Re-evaluation point: if Phase 7+ playground discovers Monaco's `monaco-languageclient` integration over WebWorker has a structural mismatch with sync `lsp-server`, the channel adapter is local to `fossil-wasm` and does not affect this decision.

---

See also `decisions/rudof-wasm.md` for the parallel Phase 0 spike outcome on the rudof WASM compatibility decision (independent but recorded in the same Phase 0 sweep).
