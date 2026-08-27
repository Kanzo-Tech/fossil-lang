# @fossil-lang/playground

The loop, in a browser tab, with no server in it:

> write a `.fossil` program → it type-checks as you type → it runs → it writes a corpus →
> you query that corpus

`examples/hello.fossil` produces the same five subjects here that `fossil run` produces
natively. That equivalence is the demo; everything else is scaffolding around it.

## Running it

The app imports three gitignored `pkg/` directories that only exist after a wasm build, so
the dependencies have to be built first. The `...` suffix is what orders that:

```bash
pnpm install --filter "@fossil-lang/playground..."
pnpm --filter "@fossil-lang/playground..." build     # cargo + wasm-bindgen + tsc + vite
pnpm --filter @fossil-lang/playground dev            # http://localhost:3300
```

On a Mac the first build needs a wasm-capable `clang` for `fossil-df-wasm`'s `zstd-sys`.
Apple's is not one; Homebrew LLVM is. `CONTRIBUTING.md` has the same two variables for the
WASM gate:

```bash
CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang \
AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar \
pnpm --filter "@fossil-lang/playground..." build
```

The failure without it is `unknown target triple 'wasm32-unknown-unknown'` out of `cc-rs`,
about four hundred lines into a dependency build, where it reads as nonsense.

## What loads, and what it costs

Measured in Chrome on the built bundle, raw bytes over the wire:

| Bundle | Raw | Gzipped | Instantiate | When |
|---|---|---|---|---|
| `fossil_wasm_bg.wasm` (checker + tokenizer) | 3.53 MB | 1.19 MB | ~20 ms | on load |
| `duckdb-eh.wasm` + worker | 32.7 MB | — | ~500 ms | on load |
| `fossil_df_wasm_bg.wasm` (DataFusion) | 22.0 MB | 6.30 MB | ~150 ms | on first Run |
| `fossil_graph_wasm_bg.wasm` (verbs) | 549 kB | 217 kB | — | never, so far |
| app JS + CSS | 438 kB | 122 kB | — | on load |

The graph WASM is emitted as an asset because `@fossil-lang/graph`'s barrel resolves it,
and it is never fetched: `openCorpus` reaches the tiles through the host's `query`
callback, and only the typed verb surface instantiates the module. If the app grows a verb
call, that row stops being free.

The executor is dynamically imported so a session that only edits never pays for it. The
checker is small enough to be eager, which is what makes check-as-you-type feel like an
editor rather than a build.

## The three files worth reading first

- `src/check.ts` — why the shape document must be **opened**, and why the CSV must be
  **introspected**, before the first check means anything.
- `src/corpus.ts` — the seam that was not real, and is. `fossil-df-wasm` runs the layout
  pass in memory now, so the browser writes the tiled tree the manifest declares rather
  than the staged one. The `retile` stand-in it carried is deleted, which is what its own
  note asked for.
- `src/example.ts` — why the program is rewritten on the way into `run` and not on the way
  into the checker.

## Known defect

Typing faster than the 120 ms debounce used to poison the checker for the rest of the
session: `updateFile` re-entered while a previous call was live throws *"recursive use of
an object detected which would lead to unsafe aliasing in rust"* out of wasm-bindgen's
`RefCell`, and the workspace never recovers. `src/check.ts` debounces and guards, which is
what an LSP client does anyway — but a host calling the surface correctly should not be
able to poison it, and the fix belongs on `crates/fossil-wasm`.

## What it is not

Not a design system. `src/styles.css` is three colours and a font stack, and it is meant to
be deleted rather than extended — `@kanzo-tech/ui` is the intended one and is not on npm.

Not an editor. A textarea that genuinely runs beats a beautiful editor that cannot execute
a program. `@fossil-lang/wasm` already exposes `tokenize` and `semanticLegend` for the
editor half, and `crates/fossil-ide` carries hover, completion and goto-def behind them.
