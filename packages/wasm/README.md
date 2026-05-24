# @fossil-lang/wasm

JS/TS wrapper around the `fossil-wasm` Rust crate's wasm-bindgen artefacts. Provides:

- `initFossilWasm({ wasmUrl })` — consumer-controlled `.wasm` URL loader (memoised).
- `tokenize(text)` — calls the Rust lexer, returns `TokenRow[]` (ADR-0030).
- `semanticLegend()` — returns the LSP semantic-tokens legend (Phase 6 plan 06-07).
- `FossilPlayground` — the Workspace API class (ADR-0024) for LSP + compile.

## Why explicit `init({ wasmUrl })` and not auto-load?

We use `wasm-bindgen --target web` (NOT `--target bundler`). This means consumers
control the `.wasm` URL resolution — works in Vite, Next.js, Webpack, Rspack, or
plain `new URL(...)` in a Web Worker context. See `08-RESEARCH.md` Pitfall 1
(this monorepo's phase-8 research) for why `--target bundler` was rejected.

## Consumer patterns

### Vite host

```typescript
import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

await initFossilWasm({ wasmUrl });
const tokens = tokenize('prefix ex: <https://example.org/>');
```

### Next.js host (in a Client Component)

```typescript
'use client';
import { initFossilWasm, tokenize } from '@fossil-lang/wasm';

// Put fossil_wasm_bg.wasm in public/wasm/ at build time (next.config.mjs copies it)
useEffect(() => {
  void initFossilWasm({ wasmUrl: '/wasm/fossil_wasm_bg.wasm' });
}, []);
```

### Web Worker

```typescript
// inside a Worker module:
import { initFossilWasm, FossilPlayground } from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

await initFossilWasm({ wasmUrl });
const pg = new FossilPlayground();
```

## Build

```bash
# From repo root
pnpm --filter @fossil-lang/wasm build:wasm   # → packages/wasm/pkg/
pnpm --filter @fossil-lang/wasm build         # = build:wasm + tsc
```

Requires: Rust 1.90 toolchain (per `rust-toolchain.toml` at repo root) +
`wasm-bindgen-cli` 0.2.120 (`cargo install --version 0.2.120 wasm-bindgen-cli`).
Optionally `wasm-opt` (binaryen) for size reduction.

## Source of truth

- Rust crate: `crates/fossil-wasm/`
- tokenize ADR: `decisions/0030-wasm-exported-tokenizer.md`
- Workspace API ADR: `decisions/0024-fossil-wasm-workspace-api.md`
- Distribution pattern: `decisions/0028-playground-as-react-library.md`
  (this package is one of the six in the `@fossil-lang/*` family)
