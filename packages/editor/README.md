# @fossil-lang/editor

> Standalone Fossil editor — CodeMirror 6 + LSP wiring with pluggable transport.

> Scaffolded in Phase 11 plan 11-01. `FossilEditor` component lands in 11-02.
> Transports (`WorkerTransport`, `HttpTransport`, `NullTransport`) land in
> 11-03. Ready for v0.2.0 minor release per Phase 11 11-05.

## Why this package

The Fossil editor was originally bundled inside `@fossil-lang/playground` as a
sub-component. Phase 11 extracts it into its own publishable package so that
any host can drop a `<FossilEditor/>` into a React 18+ tree without pulling in
the full playground shell (DuckDB-WASM, Cosmos.gl viewer, result panels, ...).

Per [ADR-0028](../../decisions/0028-playground-as-react-library.md) (multi-host
React library family), `@fossil-lang/editor` is a sibling to
`@fossil-lang/playground` — not a sub-export. Phase 16's Keasy migration
consumes `@fossil-lang/editor` directly with `HttpTransport` for backend LSP;
public hosts consume it via `@fossil-lang/playground`'s re-export with
`WorkerTransport` for browser-side LSP.

## Quick start

```tsx
import { FossilEditor, WorkerTransport } from '@fossil-lang/editor';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

// Construct a Worker running the fossil-wasm LSP (or use the playground's
// pre-built worker entry — see @fossil-lang/playground/dist/workers/).
const worker = new Worker(/* lsp.worker.js */);
worker.postMessage({ type: '__boot', wasmUrl });

const transport = new WorkerTransport({ worker });
const resolver = createDefaultResolver({ examples: [] });

<FossilEditor
  value={source}
  onChange={setSource}
  lspTransport={transport}
  resolver={resolver}
/>
```

The snippet documents the SHAPE that 11-02 (`FossilEditor`) + 11-03
(transports) implement.

## Transports

The editor accepts any object implementing the `Transport` interface (per
[ADR-0036](../../decisions/0036-transport-superset.md) — a strict superset of
`@codemirror/lsp-client`'s narrower interface). Three implementations ship:

### `WorkerTransport`

```tsx
import { WorkerTransport } from '@fossil-lang/editor';

const transport = new WorkerTransport({ worker: myWorker });
```

Wraps a Web Worker running the fossil-wasm LSP (the playground's `useLspWorker`
boot mechanics still apply). Per [ADR-0026](../../decisions/0026-worker-lifecycle.md)
the Worker is long-lived; `transport.close()` calls `worker.terminate()` —
use sparingly (typically only on tab close).

`SendOptions.signal` is documented as ignored — Workers cannot cancel
in-flight `postMessage`. Use `close()` for hard cancellation.

### `HttpTransport`

```tsx
import { HttpTransport } from '@fossil-lang/editor';

const transport = new HttpTransport({
  endpoint: '/api/fossil/analyze',
  headers: { authorization: `Bearer ${token}` },
  // fetch?: typeof fetch — useful for tests + Node environments
});
```

POSTs JSON-RPC envelopes to a configurable endpoint. Phase 16 Keasy uses this
with `endpoint: '/api/fossil/analyze'` against a backend-hosted LSP. The
constructor's `headers` slot is the seam Keasy fills with credentials —
HttpTransport itself has zero auth-aware code.

`SendOptions.signal` is honored — forwarded to the underlying `fetch()`. On
abort, the in-flight request is cancelled; no response dispatched. `close()`
aborts all in-flight requests + clears handlers (an AbortSignal.any polyfill
for Safari < 17.4 lives in `transports/Http.ts` — see ADR-0036).

### `NullTransport`

```tsx
import { NullTransport } from '@fossil-lang/editor';

const transport = new NullTransport();

<FossilEditor value={source} lspTransport={transport} /* read-only static */ />
```

No-op Transport. The editor still renders + tokenizes (via the lexer in
`@fossil-lang/codemirror-fossil`) + supports `@`-autocomplete from
`resolver`, but no LSP features fire (hover, completion, diagnostics,
semantic-tokens are inert). Use for read-only docs, examples gallery
thumbnails, or read-only audit views.

## Theming

The editor is theme-less. It consumes the `--fossil-*` CSS variable contract
documented in `@fossil-lang/ui`'s README. To get the kanzo IDE look, wrap your
app root in `<KanzoThemeProvider/>` from `@kanzo/theme`:

```tsx
import { KanzoThemeProvider } from '@kanzo/theme';

<KanzoThemeProvider>
  <FossilEditor /* ... */ />
</KanzoThemeProvider>
```

Or bring your own theme via the documented CSS-variable contract (see
[`@fossil-lang/ui`](../ui/README.md) for the full variable list).

## Bundle budget

- **Primary cap:** 100 KB gzipped (everything bundled — see
  `.size-limit.cjs`).
- **Diagnostic cap:** 20 KB gzipped (CM6 + LSP-client + lang-extension
  excluded — tracks our-own-code growth separately).

Both are enforced via `pnpm --filter @fossil-lang/editor size` in CI.

## Backwards compatibility

`@fossil-lang/playground` v0.2.x re-exports `FossilEditor` (and
`createWorkerTransport`, the v0.1 functional shim) from this package. v0.1.x
consumers — `import { FossilEditor } from '@fossil-lang/playground'` —
continue to work without code changes. Module-instance dedup via pnpm
workspace symlinks ensures referential identity.

## ADRs referenced

- [ADR-0028](../../decisions/0028-playground-as-react-library.md) — multi-host
  React library family
- [ADR-0029](../../decisions/0029-two-tier-source-resolution.md) — host-injected
  `ConnectionResolver` for `@connector/path` autocomplete
- [ADR-0030](../../decisions/0030-wasm-exported-tokenizer.md) — `tokenize()`
  exported from `fossil-wasm`; consumed by `@fossil-lang/codemirror-fossil`
- [ADR-0032](../../decisions/0032-lsp-client-choice.md) — `@codemirror/lsp-client`
  adoption (the editor's LSP wiring)
- [ADR-0036](../../decisions/0036-transport-superset.md) — Transport interface
  as superset of CM6's narrower one (locked in 11-01)
