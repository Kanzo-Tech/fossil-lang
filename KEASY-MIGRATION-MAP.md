# keasy migration map — what breaks when fossil's UI family goes

Written 2026-08-11, at the commit that deletes `packages/{ui,viewer,editor,codemirror-fossil}`.
This file is for the session that does the keasy side; it is a map, not a plan, and every line was
verified against the tree on that date.

**keasy is not broken by the deletion.** It installs from npm (`web/package.json` pins
`"@fossil-lang/viewer": "alpha"`), and `0.3.0-alpha.1..3` of all four packages are published and
permanent — npm's unpublish window is 72 h and it closed long ago. Deleting the source freezes the
names; it does not remove them. keasy keeps building until it chooses to move.

## The 11 files

### Viewer surface — 7 files, 6 symbols

| File (`keasy/web/src/`) | Line | Symbols |
|---|---|---|
| `app/(main)/(workspace)/(member)/jobs/[id]/discover/page.tsx` | 12 | `GraphCanvas`, `DEFAULT_GRAPH_CONFIG`, `CosmosGraphHandle` |
| `components/jobs/detail/catalog-view.tsx` | 8 | same |
| `components/jobs/detail/discovery-view.tsx` | 14 | same |
| `components/discovery/node-info.tsx` | 8 | `GROUP_CSS_COLORS` |
| `components/discovery/graph-settings.tsx` | 12 | `DEFAULT_GRAPH_CONFIG` |
| `components/discovery/floating-controls.tsx` | 5 | `CosmosGraphHandle` |
| `components/discovery/use-graph-data-rows.ts` | 18 | `VertexRow`, `EdgeRow` |

keasy uses **none** of the viewer's headline exports — not `FossilViewer`, not `FossilGraphView`,
not `TurtleTab`, not `TabularFallback`, not `useGraphCrossfilter`. Roughly two thirds of that
package was never consumed by anyone.

Replacements in `@kanzo-tech/graph`: `useCosmosGraph` + `useBoundedGraph` for `GraphCanvas`,
`adaptive()` for `DEFAULT_GRAPH_CONFIG` (already absorbed — `packages/graph/src/adaptive.ts:7`
cites ADR-0040 by name). **Do NOT port `GROUP_CSS_COLORS`**: eight literal hexes would fail
kanzo-ui's `no-literal-hues.test.ts` on arrival. Use `categoricalColor(i)` → `var(--chart-N)`.

The arrangement (toolbar, legend, inspector) belongs at keasy's call site, not in the package —
`packages/graph/src/index.ts:1-9` states that contract. The reference arrangement to copy from is
`kanzo-ui/docs/showcases/workspace/`.

### Editor surface — 4 files

| File | Line | Symbols | Note |
|---|---|---|---|
| `components/jobs/step-script.tsx` | 7 | `FossilEditor` | → `@kanzo-tech/ui/editor`'s `CodeEditor`, `basics={false} chrome={false}` |
| `components/jobs/assistant-wizard.tsx` | 19 | `FossilEditor` | same |
| `lib/fossil/use-fossil-lsp-transport.ts` | 23 | `createWorkerTransport`, `Transport` | protocol plumbing; moves to kanzo-ui verbatim |
| `lib/fossil/use-source-descriptors.ts` | 26-32 | `buildDescriptor`, `extractSourceRefs`, `DescribeRow`, `InferredDescriptor` | **false alarm** — pure re-exports of `@fossil-lang/introspect`, which STAYS. One-line import redirect. |

## The real work is not the imports

It is the **rows → dense buffers adapter**. `@kanzo-tech/graph`'s `memorySource` takes
`BigUint64Array` vertices and already-resolved dense links; keasy holds `{id, type, label}[]` with
string ids. fossil's `useGraphData` was that bridge and kanzo deleted it deliberately (ADR-0001 —
the id→index map cost 148-158 ms at 200k and was designed away).

**`memorySource` has no call site anywhere in kanzo-ui** — only `graph-model.test.ts:116-175`. So
the "host already holds arrays" path is shipped and unexercised, and keasy is the first host to
stand on it. Expect to find bugs there; that is where the time goes, not in the swap.

keasy's existing adapter to port/replace: `components/discovery/use-graph-data-rows.ts` (~138 lines).

## Free wins to take in the same commit

- **Delete `lib/fossil/use-fossil-wasm.ts` outright.** Its only reason to exist is a SECOND,
  main-thread WASM instantiation, needed because `tokenize()` is called synchronously off the LSP
  worker. Under `semanticTokens/full` the worker's copy is the only copy. The `wasmReady` render
  gate at `step-script.tsx:110-112` goes with it. Proof: in the browser,
  `performance.getEntriesByType('resource').filter(r => r.name.includes('fossil_wasm'))` shows the
  WASM fetched **once**, not twice.
- **Delete the `--fossil-*` token bridge**, `app/globals.css:84-130` — 26 names that exist only to
  translate fossil's tokens into keasy's. It dies with `@fossil-lang/ui`.
- **Fix a false comment:** `lib/fossil/lsp.worker.ts:10` says the worker owns `fossil/setTargetShex`.
  That method does not exist anywhere in fossil — zero hits across every `.rs` and `.ts`. The real
  table is `crates/fossil-wasm/src/lsp_worker.rs:210-220`: hover, definition, completion,
  documentSymbol, semanticTokens/full, codeAction, `fossil/checkAll`,
  `fossil/registerInferredDescriptor`. The same false line is in `Keasy-local`.

## What must exist before keasy can move

1. `@kanzo-tech/{theme,ui,graph}` published. They are `0.0.0` and 404 on npm today.
2. A **semanticTokens client**: `@codemirror/lsp-client@6.2.4` exports nothing for highlighting
   (checked its `.d.ts`), so it is a `textDocument/semanticTokens/full` request plus a 5-int delta
   decode, legend read from the `initialize` result. ADR-0032 promised this as "a thin 50-100-line
   adapter" and it was never written; until it exists, an editor on kanzo-ui has no colour.
3. Three gaps closed in `crates/fossil-ide/src/semantic.rs` first, or the new highlighting is worse
   than the old: interpolated-string tokens (`STRING_OPEN` / `STRING_TEXT` / `INTERP_OPEN` /
   `STRING_CLOSE`) all fall to `_ => None`; punctuation is uncoloured; `ty::TYPE` is declared in the
   legend and never emitted.

`Keasy-local` is a second checkout with the same 11 files at the same lines; it does not have
`@fossil-lang/*` installed, so it breaks at install/build rather than at runtime.
