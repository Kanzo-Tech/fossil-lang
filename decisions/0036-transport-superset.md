# ADR-0036: Transport Interface as Superset of @codemirror/lsp-client's Transport

**Date:** 2026-05-26
**Status:** accepted
**Decider:** Angel Iglesias Préstamo
**Cite:** `.planning/phases/11-fossil-lang-editor-extraction/11-CONTEXT.md` § "LSP transport pluggable — los tres modos"

## Context

Phase 11 ships three LSP transport implementations — `WorkerTransport`,
`HttpTransport`, `NullTransport` — in the new `@fossil-lang/editor` package
(EDIT-02). The editor's LSP wiring goes through `@codemirror/lsp-client`
v6.2.4 (per ADR-0032). That package defines its own `Transport` interface
(subset: `send`/`subscribe`/`unsubscribe` — see
`node_modules/@codemirror/lsp-client/dist/index.d.ts` line 170):

```typescript
type Transport = {
  send(message: string): void;
  subscribe(handler: (value: string) => void): void;
  unsubscribe(handler: (value: string) => void): void;
};
```

Two architectural questions stood between us and shipping 11-02 + 11-03:

1. **Adopt CM6's Transport directly, OR define our own?** Our `HttpTransport`
   needs `AbortSignal` for cancellation (Keasy backend Phase 16; long-running
   queries must be abortable). CM6's interface has no cancellation hook.
2. **Lifecycle:** `close()` to terminate a Worker / abort in-flight HTTP. CM6's
   interface has no lifecycle hook either (their assumption: the Transport is
   constructed once at `LSPClient` boot and lives forever).

These two gaps make CM6's interface insufficient for `@fossil-lang/editor`
shipping standalone — we want consumers to swap transports at runtime
(e.g., Keasy stops + restarts an HTTP session when the user switches
connections; the playground terminates the Worker on tab close per ADR-0026).

A third concern is forward compatibility with `AbortSignal.any()` — the
canonical API for composing multiple signals — which ships in Safari 17.4+
(2024-03). Older Safari requires a polyfill.

## Decision

We will define our own `Transport` interface in
`packages/editor/src/transports/types.ts` (LOCKED in plan 11-01) as a
STRICT SUPERSET of `@codemirror/lsp-client`'s Transport:

```typescript
export interface SendOptions {
  signal?: AbortSignal;
}

export interface Transport {
  send(msg: string, opts?: SendOptions): void;            // opts is OURS
  subscribe(handler: (msg: string) => void): void;         // verbatim CM6
  unsubscribe(handler: (msg: string) => void): void;       // verbatim CM6
  close?(): void;                                          // OUR ADDITION
}
```

The key properties:

- **Strict superset:** `send`/`subscribe`/`unsubscribe` byte-for-byte match
  CM6. The additions are `opts?: SendOptions` (optional second param) and
  `close?(): void` (optional method). A Fossil `Transport` instance therefore
  ALSO satisfies CM6's narrower interface via structural typing — we pass
  our transport directly to `new LSPClient(...).connect(transport)`;
  TypeScript narrowing happens at the boundary.
- **`SendOptions.signal: AbortSignal`** — `HttpTransport` honors (forwards
  to `fetch`); `WorkerTransport` documents the ignore (Workers cannot cancel
  in-flight `postMessage`); `NullTransport` no-op.
- **`close?(): void`** — `HttpTransport` aborts in-flight requests + clears
  handlers; `WorkerTransport` removes the message listener + terminates the
  Worker + clears handlers; `NullTransport` no-op.

**Polyfill: `AbortSignal.any()` for Safari < 17.4.** `HttpTransport`
composes the caller's per-request `AbortSignal` with its own internal
`AbortController` (for `close()`). `AbortSignal.any([a, b])` is the canonical
API but ships in Safari 17.4+ only. For older Safari, we polyfill via
`new AbortController()` + `addEventListener('abort')` listeners on both source
signals. The polyfill (~10 LOC) lives in `packages/editor/src/transports/Http.ts`
(11-03 ships it); not a separate dep.

## Consequences

**Positive:**

- Consumers can write custom transports (e.g., WebSocket, BroadcastChannel)
  against our interface — they only need to satisfy `Transport`. No
  CM6-specific knowledge required.
- Cancellation is a first-class concern — Phase 16 Keasy can pass
  user-driven `AbortSignal`s through `FossilEditor` →
  `HttpTransport.send(msg, { signal })`.
- `close()` lifecycle is explicit — `FossilEditor` consumers can swap
  transports at runtime without leaking Workers or in-flight HTTP requests.

**Negative:**

- Divergence from upstream CM6 interface — if `@codemirror/lsp-client` adds
  `signal` or `close()` to its own Transport in a future release, we may need
  to reconcile (revisit this ADR).
- The `AbortSignal.any` polyfill adds ~10 LOC + carries forward-compat risk
  — when we drop Safari < 17.4 support (likely Phase 17 release-prep), the
  polyfill becomes dead code, but cheap to remove.

**Neutral:**

- Class-based instantiation (`new HttpTransport({...})`,
  `new WorkerTransport({...})`) is the new contract; a thin functional shim
  `createWorkerTransport(worker)` is preserved for v0.1.x backwards
  compatibility (the playground's `useLspWorker.ts` keeps its current call
  site).
- The interface is locked in 11-01 (THIS plan) so 11-02 and 11-03 can run in
  parallel against a binding contract.

## Cross-references

- Supersedes the implicit transport assumption in [ADR-0032](0032-lsp-client-choice.md)
  (which said "use @codemirror/lsp-client" without specifying the transport
  boundary).
- Referenced by [ADR-0026](0026-worker-lifecycle.md) — `close()` defines the
  editor-side lifecycle hook the Worker policy depends on.
- Locks the contract that `packages/editor/src/transports/{Worker,Http,Null}.ts`
  implement (11-03) and that `packages/editor/src/FossilEditor.tsx`'s
  `lspTransport: Transport | null` prop accepts (11-02).
