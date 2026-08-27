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

## The editor

`@kanzo-tech/ui`'s `CodeEditor` is the host — CodeMirror 6, chrome, keymaps, search
panel, theme — and `@fossil-lang/codemirror-fossil` is the language inside it:
highlighting from `tokenize()` + `tokenKinds()`, squiggles from `check()`. Neither
package knows about the other. `CodeEditor` takes `extensions` in a
live-reconfigured `Compartment` and externalises every `@codemirror/*` as an
optional peer; the language layer asks whichever `HighlightStyle` is installed for
its classes, so fossil arrives in kanzo's palette without either side arranging it.

`@kanzo-tech/ui` **is not on npm and this app does not wait for it.** The specifier
is a tarball committed at `vendor/kanzo-tech/`, and `vendor/kanzo-tech/README.md`
has the three routes that do not survive a fresh clone — a workspace spanning both
repositories, `pnpm link`, and a `file:`/git URL at `kanzo-ui/packages/ui` — with
the manifest facts that stop each one.

What the editor does NOT have is hover, completion and goto-definition. All three
exist in `crates/fossil-ide` and `crates/fossil-wasm`'s `lsp_worker.rs` dispatches
them, but over `postMessage` from a Worker — an LSP client is the work, and it is
its own piece.

## The defect that used to be here

Typing faster than the debounce poisoned the checker for the rest of the session:
`updateFile` re-entered while a previous call was live threw *"recursive use of an
object detected which would lead to unsafe aliasing in rust"* out of
wasm-bindgen's exclusive borrow, and because a panic on wasm32 aborts without
unwinding, the borrow flag was never cleared and every later keystroke failed the
same way. This app carried a 120 ms `setTimeout` and a `busy` flag to stay clear
of it.

Both halves moved to where they belong. `crates/fossil-wasm` exports the workspace
through a type holding it in its own `RefCell` with every method taking `&self`, so
re-entry returns a catchable error naming the method and the workspace still works
afterwards. And the coalescing is `@fossil-lang/codemirror-fossil`'s linter, which
waits out its delay and then waits for the check to return before scheduling the
next — an LSP client's `didChange` behaviour, in the library rather than in a
convention this app had to remember.

## What it is not

Not a design system, and not one of its own any more. `src/styles.css` is three
colours and a font stack layered over `@kanzo-tech/ui/styles.css` — which ships
PREBUILT, a Tailwind v4 pass run in that repo over its own source plus
`@kanzo-tech/theme`'s tokens. So Tailwind is its devDependency and not this app's:
there is no Tailwind here, no PostCSS, no config, and one import in `main.tsx`.

Not a graph viewer. `@kanzo-tech/graph` is deliberately NOT adopted: it
hard-requires `@cosmos.gl/graph`, three `@uwdata/*`, and
`@duckdb/duckdb-wasm@^1.33.1-dev57.0` — a pre-release pin that does not match the
`1.32.0` this app already loads. That is a separate decision needing its own
argument. (`@kanzo-tech/ui` names the same duckdb pin, but as an OPTIONAL peer used
only by its `/analytics` subpath, which nothing here imports; `pnpm install` warns
about it and the warning is correct and inert.)
