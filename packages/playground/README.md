# @fossil-lang/playground

Embeddable React playground for Fossil — CodeMirror editor + LSP via WASM Worker + DuckDB-WASM runner + Mosaic/Cosmos.gl result viz.

Per ADRs 0024 (fossil-wasm Workspace API), 0026 (Two Workers, Two Lifecycles), 0027 (CodeMirror over Monaco), 0028 (playground as React library), 0029 (two-tier source resolution / ConnectionResolver), 0030 (WASM-exported tokenizer), 0032 (`@codemirror/lsp-client` adoption).

## Quick start

```bash
pnpm create vite my-host --template react-ts
cd my-host
pnpm add @fossil-lang/playground @fossil-lang/wasm @fossil-lang/codemirror-fossil \
         @fossil-lang/resolvers @fossil-lang/examples \
         @duckdb/duckdb-wasm \
         @codemirror/state @codemirror/view @codemirror/language \
         @codemirror/autocomplete @codemirror/lint
```

```tsx
import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import { buildResolverExamples } from '@fossil-lang/examples';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

const resolver = createDefaultResolver({ examples: buildResolverExamples() });

export default function App() {
  return <FossilPlayground resolver={resolver} wasmUrl={wasmUrl} />;
}
```

## Architecture

- **Editor:** CodeMirror 6 via `@fossil-lang/codemirror-fossil` (ADR-0027)
- **LSP:** Web Worker hosting `@fossil-lang/wasm` (ADR-0024) via `@codemirror/lsp-client` (ADR-0032)
- **Execution:** DuckDB-WASM, lazy-loaded, terminate+recreate on Reset (ADR-0026 + PLAY-12)
- **Source resolution:** Two-tier — host injects `resolver: ConnectionResolver` (ADR-0029); Tier-1 default in `@fossil-lang/resolvers`
- **No host-side credential exposure:** CONN-01 invariant test guards this — `tests/no-credentials-leak.test.tsx` walks the rendered DOM and grepps for credential-shape strings

## Reset semantics

The "Reset playground" button terminates the DuckDB Worker but NOT the LSP Worker. The editor session stays warm; the DuckDB heap is reclaimed. Per ADR-0026 (Two Workers, Two Lifecycles).

**DO NOT add a `resetLsp` API without a follow-up ADR superseding 0026.** The asymmetric API IS the enforcement mechanism.

## Peer dependencies

All `@codemirror/*` packages, `react`, `react-dom`, `@fossil-lang/wasm`, and `@fossil-lang/codemirror-fossil` are declared as `peerDependencies`. `@duckdb/duckdb-wasm@1.32.0` is also a peer (optional — required only when the Run path is exercised). Hosts MUST install matching versions to avoid the duplicate-state crash documented in RESEARCH.md Pitfall 10.

## Bundle budgets (PKG-03)

- Core gzip < 500 KB (excluding WASM) — verified by `@fossil-lang/playground`'s position in the workspace bundle-size CI (08-03)
- WASM total < 2 MB compressed — enforced by `.github/workflows/wasm-size.yml`

## Hooks + sub-component exports

- `<FossilEditor />` — hand-rolled React wrapper around `EditorView` (~50 LOC)
- `<ResultTable />` — TanStack Table fallback for tabular results
- `<ResultGraph />` — Cosmos.gl WebGL graph view with tabular fallback
- `useLspWorker({ wasmUrl })` — module-singleton LSP Worker + `LSPClient`
- `useDuckDb()` — lazy-load `@duckdb/duckdb-wasm` + Run hook
- `useResetPlayground()` — ADR-0026 reset (DuckDB only)
- `getDuckDb()`, `resetDuckDb()` — module-scope DuckDB helpers
- `createWorkerTransport(worker)` — adapt a Worker to `@codemirror/lsp-client`'s `Transport`
