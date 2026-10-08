# @fossil-lang/wasm

The fossil compiler front end in the browser, over the `fossil-wasm` Rust crate's wasm-bindgen
artefacts. Every door boots the module itself:

- `openProgram(uri, { host, text, signal })` — one program open for an editor. See below.
- `inputs(text, { host, signal })` — what a program reads, with no editor: TypeScript's
  `ts.preProcessFile`. Each `Input` is `{ role, key, location, connection?, binding?, format?,
  option? }` — a data source or a shape document, the key as written and the location through
  `host.connections()`. It reads no document, and it never touches an editor's program.
- `formats({ signal })` and `formatFor(path, role, formats)` — the formats this build reads, and
  which of them reads a file as `'data'` or as `'schema'`, by extension. `formatFor` is pure: a host
  asks the module for the list once and filters a whole listing with it. The role is required
  because one extension names two formats — a `.ttl` is `rdf` as data and `shacl` as a schema.
- `initFossilWasm(bytes)` — Node only: hand over the `.wasm`. A bundled host never calls it.

## One program in one editor: `openProgram`

```typescript
import { openProgram } from '@fossil-lang/wasm';
import { fossil } from '@fossil-lang/codemirror-fossil';

const program = await openProgram('job.fossil', { host, text });   // host: Host
const extensions = fossil(program, { onNavigate });

program.registerIntrospection(await introspect(await program.inputs(), { host, engine }));
```

It boots the module, opens `uri` in its own workspace, and reads the documents the text names
through `host`. The text arrives through `update(text)` — LSP's `didChange`, which `fossil()` calls
from the view's update cycle — and every question takes only a position: `hover(line, character)`,
`completion(…)`, `definition(…)`, `semanticTokens()`. `diagnostics()` and `inputs()` first run
`resolveDocuments`, which reads nothing when nothing is missing. `close()` (or `using`) frees it.

A program names shape documents (`io.shex("@vocab/person.shex")`) and data sources
(`io.csv("@lake/users.csv")`). The checker reads no file and no network: `resolveDocuments`
(`@fossil-lang/storage`) reads what it is missing under the credentials the `Host`
(`@fossil-lang/types`) vends — `connections()` and `credentials(scope, access)`. A document under a
connection is read by a GET signed with that connection's `read` credential; one with no connection
only when it is a public `http(s)` URL.

## Loading: the `.wasm` is an asset of this package

A bundled host imports and calls a door, and that is the whole host flow, in Vite, Next.js (webpack or Turbopack), a Web
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

A workspace-linked package is never pre-bundled, which is why a host inside this
repository would need no such line. Next.js needs none in either `next dev` or `next build`, webpack or
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
