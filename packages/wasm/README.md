# @fossil-lang/wasm

JS/TS wrapper around the `fossil-wasm` Rust crate's wasm-bindgen artefacts. Provides:

- `initFossilWasm()` — boots the module (memoised). Its `.wasm` ships in this
  package and the host's bundler emits it as an asset; the host copies nothing.
- `tokenize(text)` — calls the Rust lexer, returns `TokenRow[]`. The Rust lexer
  is the only lexer: no host reimplements one and drifts from the grammar.
- `semanticLegend()` — returns the LSP semantic-tokens legend.
- `FossilPlayground` — the Workspace API class for LSP + compile. `fossil-lsp`
  itself is native-only (stdio over crossbeam), so the browser gets this
  equivalent dispatch surface over the same `fossil-ide` functions.

## Documents and sources: fossil resolves, the host reads

A program names shape documents (`io.shex("@vocab/person.shex")`) and data
sources (`io.csv("@lake/users.csv")`). The checker reads no file and no
network: it reports what it is missing, and the host hands back text through a
`SourceHost` (`@fossil-lang/types`) — the connection map, and `sign(locators)`.

```typescript
import { resolveDocuments } from '@fossil-lang/types';

const pg = new FossilPlayground();
const h = pg.openFile('prog.fossil', text); // edited buffers only
const { unread } = await resolveDocuments(pg.workspace(h), host); // sets the connections too
const rows = pg.check();
```

- `missingDocuments(h)` — `{ key, locator }` rows. The key is what the program
  wrote, so repointing a connection invalidates nothing; the locator is that
  key through the map, and it is what `sign` receives.
- `registerDocument(key, text)` — what `resolveDocuments` calls for each
  fetched document, until nothing new is missing (a document can name another).
  It is the one loop; a host does not write a second.
- `openFile` is for buffers the user edits. An open `.shex` is the document
  every program naming it reads; opening a program registers nothing it names.
- `setConnections(map)` — name → base; re-checks nothing. `resolveDocuments`
  calls it with `host.connections()`, so a host never does.
- `sources(h)` — the `ProgramSource[]` the program reads, through the map the
  last `resolveDocuments` set: binding, key,
  locator, catalogue row, reader option. Introspection DESCRIBEs these and
  registers each descriptor under `key` with `registerInferredDescriptor`.

## Loading: the `.wasm` is an asset of this package

```typescript
import { initFossilWasm, tokenize } from '@fossil-lang/wasm';

await initFossilWasm();
const tokens = tokenize('User := io.csv("data/people.csv")');
```

That is the whole host flow, in Vite, Next.js (webpack or Turbopack), a Web
Worker or any bundler that understands `new URL('…', import.meta.url)`. The glue
(`wasm-bindgen --target web`) locates `fossil_wasm_bg.wasm` with exactly that
expression, the bundler copies the file into its output under a hashed name and
rewrites the URL, and the browser fetches it from there. No copy script, no
`public/` directory, no URL to keep in sync with a version.

**Vite dev server, package installed from npm:** Vite's dependency optimizer
pre-bundles the package into `node_modules/.vite/deps/` without its `.wasm`, and
the URL then answers with `index.html`. Keep the three packages out of it —
`vite build` needs nothing:

```js
// vite.config.js
export default { optimizeDeps: { exclude: ['@fossil-lang/wasm', '@fossil-lang/executor', '@fossil-lang/corpus'] } };
```

A workspace-linked package is never pre-bundled, which is why the playground needs
no such line. Next.js needs none in either `next dev` or `next build`, webpack or
Turbopack.

`--target bundler` was rejected because it emits `import … from '*.wasm'` (the
ESM-integration proposal), which each bundler gates behind its own experimental
flag. `new URL(…, import.meta.url)` is the pattern they all support by default.

**Without a bundler**, pass the module yourself — `initFossilWasm(wasm)` takes the
glue's `InitInput` (`BufferSource`, `Response`, `URL`, `WebAssembly.Module`). Node
is the case: its `fetch` rejects `file://`, so hand it the bytes:

```typescript
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

const path = createRequire(import.meta.url).resolve('@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm');
await initFossilWasm(await readFile(path));
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

- Rust crate: `crates/fossil-wasm/` — everything here is a wrapper over its
  wasm-bindgen exports, and nothing in this package reimplements it.
- The grammar the tokenizer follows: `grammar.bnf` at the repo root.
