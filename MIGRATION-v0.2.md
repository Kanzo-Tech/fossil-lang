# Migrating to `@fossil-lang/* v0.2.0`

> Status: reference. v0.2.0 is the structural release — the `@fossil-lang/*` package family is complete (6 pre-existing packages + 2 new: `editor`, `viewer`). This guide is for adopters with a Keasy-like in-tree pattern (your own `<CodeEditor/>` + graph integration) who want to extract to the shared library. If you're new to Fossil, start at the playground: <https://playground.kanzo.dev> — you don't need this guide.

## Table of contents

1. [Quickstart](#quickstart)
2. [Migration recipe](#migration-recipe)
3. [Per-package consumption](#per-package-consumption)
   - [@fossil-lang/types](#fossil-langtypes)
   - [@fossil-lang/resolvers](#fossil-langresolvers)
   - [@fossil-lang/codemirror-fossil](#fossil-langcodemirror-fossil)
   - [@fossil-lang/wasm](#fossil-langwasm)
   - [@fossil-lang/examples](#fossil-langexamples)
   - [@fossil-lang/editor](#fossil-langeditor) (NEW in v0.2)
   - [@fossil-lang/viewer](#fossil-langviewer) (NEW in v0.2)
   - [@fossil-lang/playground](#fossil-langplayground)
4. [Transport decision tree](#transport-decision-tree)
5. [CSS theming integration](#css-theming-integration)
6. [Troubleshooting](#troubleshooting)
7. [Live demos + further reading](#live-demos--further-reading)

## Quickstart

**Existing v0.1.x consumers** (you already use `@fossil-lang/playground` or the 6 v0.1 packages):

```bash
pnpm up '@fossil-lang/*@^0.2.0'
```

Strict semver — the upgrade is drop-in for the 6 pre-existing packages. No code changes required unless you want to adopt the two new packages (`@fossil-lang/editor`, `@fossil-lang/viewer`); if you do, read the [Migration recipe](#migration-recipe) below.

**New adopters** (you're migrating off an in-tree `<CodeEditor/>` + graph integration):

```bash
pnpm add @fossil-lang/editor @fossil-lang/viewer @fossil-lang/playground @fossil-lang/resolvers
```

Peer dependencies you must already have (the packages will warn loudly if not):

| Peer            | Version             |
| --------------- | ------------------- |
| `react`         | `^18.3.0` or `^19`  |
| `react-dom`     | `^18.3.0` or `^19`  |
| `@codemirror/state` / `view` / `language` / `autocomplete` / `lint` | `^6.0.0` |
| `@codemirror/lsp-client` | `^6.2.4`   |

Three-line mount of the composition root (fastest path to a working playground):

```tsx
import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url'; // Vite

const resolver = createDefaultResolver();
export const Demo = () => <FossilPlayground resolver={resolver} wasmUrl={wasmUrl} />;
```

If you only need the primitives (you have your own layout shell), skip the playground entry and consume `<FossilEditor/>` + `<FossilViewer/>` directly — see their per-package sections.

## Migration recipe

This recipe is targeted at adopters with an in-tree Fossil-aware editor and an in-tree Cosmos.gl-based graph view (the Keasy pattern). If you only have one of the two, run only the relevant steps. Each step ends with a "Keasy reference" link to the literal Phase 16 commit on the `fossil-migration` branch — those commits are the canonical case study for this migration.

### Step 1: Identify your in-tree components

Locate the modules you want to delete:

```bash
# Editor candidates: in-tree CodeMirror + Fossil language extension.
grep -rln 'fossilLanguage\|fossilAutocomplete\|StreamParser\|@codemirror/state' src/ \
  | grep -v node_modules \
  | grep -v '\.test\.'

# Graph candidates: in-tree Cosmos.gl or graph-view modules.
grep -rln 'cosmos.gl\|CosmosGraph\|graph-view' src/ \
  | grep -v node_modules
```

This is necessary but not sufficient. **Sibling imports are invisible to grep-by-alias.** If your in-tree files use TypeScript path aliases (`@/components/...`), a sibling file inside the same directory will import via a relative path (`./cosmos-graph`) — that path will NOT match an alias-pattern grep. Phase 16 plan 16-03 discovered two such cases (`floating-controls.tsx` and `graph-settings.tsx`) only after running `tsc --noEmit` against the post-deletion tree. See the [Troubleshooting](#tsc-surfaced-unexpected-import-errors-after-deletion) entry on this gotcha.

Recommended grep that also catches the sibling case (substitute your dirname):

```bash
grep -rln 'discovery/cosmos-graph\|discovery/graph-view\|\./cosmos-graph\|\./graph-view' src/
```

Keasy reference: this is the same diagnostic used in the Phase 16 close. The plan-context grep missed `floating-controls.tsx` and `graph-settings.tsx`; the wider variant above catches them.

### Step 2: Install `@fossil-lang/*` + plumb peer deps

```bash
pnpm add @fossil-lang/editor @fossil-lang/viewer @fossil-lang/types @fossil-lang/resolvers
```

If you already have a CodeMirror 6 setup (existing in-tree editor), your peer deps are likely correct. Verify with:

```bash
pnpm why @codemirror/state
pnpm why @codemirror/lsp-client
```

If you're vendoring pre-publish tarballs (the Phase 16 mechanism — `pnpm pack` + `file:` protocol), you ALSO need a `pnpm.overrides` block; see the [Troubleshooting](#why-was-there-a-pnpmoverrides-block-in-some-early-adopter-setups) entry. With the published v0.2.0 packages, no overrides are required.

If your host product has a Keasy-style design system (shadcn/Tailwind), also plan for the CSS bridge described in [CSS theming integration](#css-theming-integration) — without it, the editor + viewer will render against browser defaults rather than your palette.

### Step 3: Replace your in-tree `<CodeEditor/>` with `<FossilEditor/>`

The Keasy migration deleted 372 LOC of in-tree CodeMirror + language-extension code and replaced it with a thin `<FossilEditor/>` mount. The transport is `HttpTransport` because Keasy has a server backend that hosts the LSP route; if your app is browser-only, use `WorkerTransport` instead (see [Transport decision tree](#transport-decision-tree)).

Literal Keasy diff (commit `1672fe6` on `fossil-migration` branch — `feat(fossil-16-04): swap CodeEditor → FossilEditor; delete in-tree code-editor.tsx`):

```diff
- import { useRef, useMemo } from "react";
- import {
-   CodeEditor,
-   fossilLanguage,
-   fossilAutocomplete,
-   fossilLinterExtension,
- } from "@/components/discovery/code-editor";
- import type { Connection, ProviderInfo, FileEntry, FossilCompletionItem, CreationMode } from "@/lib/types";
- import type { Extension } from "@codemirror/state";
+ import { useMemo } from "react";
+ import { FossilEditor, HttpTransport } from "@fossil-lang/editor";
+ import type {
+   ConnectionResolver,
+   Connector,
+   ResolvedSource,
+   SourceRef,
+ } from "@fossil-lang/types";
+ import type { Connection, ProviderInfo, CreationMode } from "@/lib/types";
  // ... rest of imports
```

Mount-site swap:

```diff
- <CodeEditor
+ <FossilEditor
    value={script}
    onChange={onScriptChange}
-   extensions={fossilExtensions}
-   placeholder="Write your Fossil script here..."
+   lspTransport={lspTransport}
+   resolver={resolver}
    className="flex-1"
  />
```

`lspTransport` is memoised once at mount (the editor's mount effect uses it as a dep):

```typescript
const lspTransport = useMemo(
  () => new HttpTransport({ endpoint: "/v1/fossil/lsp" }),
  [],
);
```

Synthetic minimal example — same idea stripped of Keasy's `Connection[]`/`ProviderInfo[]` adapter:

```tsx
import { FossilEditor, HttpTransport } from '@fossil-lang/editor';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import { useMemo, useState } from 'react';

export function ScriptEditor() {
  const [value, setValue] = useState('');
  const transport = useMemo(() => new HttpTransport({ endpoint: '/api/fossil/lsp' }), []);
  const resolver = useMemo(() => createDefaultResolver(), []);
  return (
    <FossilEditor value={value} onChange={setValue} lspTransport={transport} resolver={resolver} />
  );
}
```

Keasy reference: commit `1672fe6` (editor swap) + commit `1ae49d9` (the matching server-side `/v1/fossil/lsp` JSON-RPC route the HttpTransport targets).

### Step 4: Replace your in-tree graph with `<FossilViewer/>`

The Keasy migration deleted 335 LOC of in-tree Cosmos.gl + `<GraphCanvas/>` duplicates and replaced them with `@fossil-lang/viewer`'s `GraphCanvas`, bridged by a 138-LOC `useGraphDataRows` adapter that materializes Keasy's Mosaic-coordinator query results into the viewer's string-id row API.

Literal Keasy diff (commit `7e8146c` on `fossil-migration` branch — `feat(fossil-16-03): swap 3 discovery consumers to @fossil-lang/viewer`):

```diff
- import { GraphCanvas, DEFAULT_GRAPH_CONFIG } from "@/components/discovery/graph-view-v2";
- import type { CosmosGraphHandle } from "@/components/discovery/cosmos-graph";
+ import { GraphCanvas, DEFAULT_GRAPH_CONFIG, type CosmosGraphHandle } from "@fossil-lang/viewer";
+ import { useGraphDataRows } from "@/components/discovery/use-graph-data-rows";
```

```diff
+ const graphRows = useGraphDataRows(kgSchema);
  // ...
- <GraphCanvas
-   schema={kgSchema}
-   graphConfig={graphConfig}
-   graphRef={graphRef}
-   selection={selection}
-   onSelectVertex={setSelectedVertex}
- />
+ {graphRows ? (
+   <GraphCanvas
+     vertices={graphRows.vertices}
+     edges={graphRows.edges}
+     graphConfig={graphConfig}
+     graphRef={graphRef}
+     selection={selection}
+     onSelectVertex={setSelectedVertex}
+   />
+ ) : (
+   <div className="flex-1 flex items-center justify-center text-sm text-muted-foreground">
+     Loading graph…
+   </div>
+ )}
```

The viewer's `GraphCanvas` is coordinator-agnostic (per ADR-0028 + Phase 12 design) — it takes raw `VertexRow[]` + `EdgeRow[]`, not a Mosaic query. If you have a query coordinator (Mosaic, ApacheArrow, your own), keep it in your consumer and materialize to the viewer's row shape in an adapter hook. If you don't, skip the adapter and pass rows directly.

Synthetic minimal example — no Mosaic, no `useGraphDataRows`:

```tsx
import { FossilViewer } from '@fossil-lang/viewer';
import type { VertexRow, EdgeRow } from '@fossil-lang/viewer';

export function GraphPanel({ vertices, edges }: { vertices: VertexRow[]; edges: EdgeRow[] }) {
  return <FossilViewer vertices={vertices} edges={edges} defaultTab="graph" />;
}
```

Keasy reference: commit `7e8146c` (viewer swap + 335-LOC deletion) + commit `86056aa` (the `useGraphDataRows` adapter + pre-publish `pnpm.overrides`).

### Step 5: Delete in-tree files + reconcile relative imports

Delete the in-tree modules:

```bash
git rm src/components/discovery/cosmos-graph.tsx
git rm src/components/discovery/graph-view-v2.tsx
git rm src/components/discovery/code-editor.tsx
```

Then run TypeScript to catch sibling-relative imports the alias-pattern grep missed:

```bash
pnpm exec tsc --noEmit
```

Phase 16 found two such consumers (`floating-controls.tsx` imported `CosmosGraphHandle` from `./cosmos-graph`; `graph-settings.tsx` imported `DEFAULT_GRAPH_CONFIG` from `./graph-view-v2`). Both are mechanical redirects to `@fossil-lang/viewer`:

```diff
- import type { CosmosGraphHandle } from "./cosmos-graph";
+ import type { CosmosGraphHandle } from "@fossil-lang/viewer";
```

If your floating-controls call `graphRef.current.zoom(factor, duration)`, that API was deliberately removed in v0.2 — see [Troubleshooting](#my-fossilviewer-floating-controls-broke-after-upgrade).

Keasy reference: both fixups landed in the same commit as the viewer swap (`7e8146c`), under Phase 16's Rule 3 (Blocking) — the deletion directly caused the tsc failures.

## Per-package consumption

### @fossil-lang/types

> **Runtime-free.** Zero JS at runtime — only TypeScript types. Safe to depend on transitively even from server code (Node SSR, edge workers, RSC).

Public surface:

```typescript
import type {
  // Source resolution (ADR-0029 IoC contract)
  SourceRef,
  ResolvedSource,
  SourceSchema,
  ConnectorType, // 'local_file' | 'public_http' | 'upload' | 'examples'
  Connector,
  ResolverEvent,
  ConnectionResolver,
  // Theme
  FossilTheme,
  FossilThemeName, // 'light' | 'dark'
  FossilThemeProp,
  // Diagnostics + tokens (LSP / wasm side)
  Diagnostic,
  TokenRow,
  SemanticTokensLegend,
} from '@fossil-lang/types';
```

Most-used type is `ConnectionResolver` — the IoC contract by which the host injects connector resolution. Hosts implement the interface and pass the instance to `<FossilEditor/>` + `<FossilPlayground/>`:

```typescript
const myResolver: ConnectionResolver = {
  async list(): Promise<Connector[]> {
    return [{ name: 'demo', type: 'public_http', label: 'Demo data' }];
  },
  async resolve(ref: SourceRef): Promise<ResolvedSource> {
    return { url: `https://example.com/${ref.path}`, format: 'csv' };
  },
};
```

`SourceRef` is the parsed `@connector/path` shape (raw text after `@`, validated against `/^[a-z0-9][a-z0-9_-]*$/i` per ADR-0029). `ResolvedSource.url` is what the component substitutes into codegen'd SQL at run time — components NEVER see plaintext credentials.

### @fossil-lang/resolvers

Three built-in `ConnectionResolver` implementations:

| Factory                       | When                                                                       |
| ----------------------------- | -------------------------------------------------------------------------- |
| `createDefaultResolver()`     | Tier 1 — bundled examples + uploads + public CORS HTTPS. The playground default. |
| `createMockResolver(connectors)` | Tests, Storybook, isolation. You hand-roll the connector list.          |
| `createPublicHttpResolver(spec)` | Pure public-CORS HTTP. Useful for static dashboards.                     |

Synthetic minimal:

```tsx
import { createDefaultResolver, createMockResolver } from '@fossil-lang/resolvers';

// Production-shape resolver — handles uploads, examples bundle, public HTTPS.
const resolver = createDefaultResolver();

// Test-shape resolver — supply your own connector list.
const mock = createMockResolver({
  connectors: [{ name: 'demo', type: 'examples', label: 'Demo' }],
  resolve: async (ref) => ({ url: `mock://${ref.connector}/${ref.path}`, format: 'csv' }),
});
```

Hosts with their own credential vault (Keasy-style closed-source SaaS) implement `ConnectionResolver` directly rather than wrapping one of the factories — the `list()` method advertises connectors, and `resolve()` returns presigned URLs at execution time. See the [Keasy step-script](#step-3-replace-your-in-tree-codeeditor-with-fossileditor) `useKeasyResolver` snippet for the host-mediated pattern.

### @fossil-lang/codemirror-fossil

Standalone CodeMirror 6 language extension for Fossil. No React, no LSP machinery — just syntactic highlighting + `@`-prefixed connector autocomplete. Use this if you have a CodeMirror editor that ISN'T `<FossilEditor/>` (e.g. you're embedding into Monaco-like surface, or you've already built your own React wrapper).

Public surface:

```typescript
import {
  fossil,                  // composition root: [language, autocomplete]
  fossilLanguage,          // language definition only
  fossilLanguageSupport,   // language + indent/fold support
  fossilStreamParser,      // raw StreamParser → fossil-wasm tokenize bridge
  fossilAutocomplete,      // @-prefix completion source
  fossilAutocompleteSource,
  FossilKind,              // syntactic tag enum
} from '@fossil-lang/codemirror-fossil';
```

Synthetic minimal:

```tsx
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { fossil } from '@fossil-lang/codemirror-fossil';
import { createDefaultResolver } from '@fossil-lang/resolvers';

const resolver = createDefaultResolver();
const state = EditorState.create({
  doc: 'prefix ex: <https://example.org/>',
  extensions: [...fossil({ resolver })],
});
new EditorView({ state, parent: document.getElementById('editor')! });
```

For LSP-driven features (diagnostics, hover, semantic-tokens overlay), compose this package's `fossil()` extension with `@codemirror/lsp-client` directly — or skip the manual composition and consume `<FossilEditor/>` from `@fossil-lang/editor`, which does this for you.

### @fossil-lang/wasm

Thin TypeScript wrapper around the `fossil-wasm` `wasm-bindgen` artifacts. Hosts the LSP server-side (per ADR-0024) — the playground runs this inside a Web Worker; native tooling can consume it as a plain module.

Public surface:

```typescript
import {
  initFossilWasm,    // consumer-controlled .wasm URL loader (memoised)
  tokenize,          // lexer → TokenRow[] (per ADR-0030)
  semanticLegend,    // returns the LSP SemanticTokensLegend
  FossilPlayground,  // Workspace API class (ADR-0024)
  startLspWorker,    // installs onmessage handler — Worker scope only
} from '@fossil-lang/wasm';
```

Synthetic minimal — Vite host loading the wasm asset by URL:

```typescript
import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

await initFossilWasm({ wasmUrl });
const tokens = tokenize('prefix ex: <https://example.org/>');
```

**Lazy-load pattern (Phase 15 plan 15-04 — BUG-02).** The wasm module is ~hundreds of KB; cold-loading it on first render is wasteful for hosts that may never invoke compilation. The playground's `<ResultGraph/>` defers the viewer-chunk import via `React.lazy`, and the wasm loader is memoised so re-loads dedupe. Apply the same pattern in your host:

```typescript
// Lazy: only initialize when the user actually invokes compile/tokenize.
const FossilTools = React.lazy(() => import('./fossil-tools'));
```

For Next.js or RSC hosts: the wasm-bindgen output uses `--target web`, which requires DOM globals. Mount inside a `'use client'` boundary, never call from server code.

### @fossil-lang/examples

Bundled `.fossil` / CSV / CSVW JSON-LD / ShEx fixtures used by the playground and reusable by any consumer wanting the same examples. Tree-shakeable — fixtures inline at consumer-bundle time via Vite `?raw` imports (no separate `fetch()`s, OFFLINE-01 compatible).

Public surface:

```typescript
import { manifest } from '@fossil-lang/examples';
import type { Example, ExampleFile } from '@fossil-lang/examples';
// Bundled examples (6): hello, hello-no-csvw, ecommerce, musicbrainz,
// typing-showcase, multi-source-join.
import { helloExample } from '@fossil-lang/examples/hello';
```

Synthetic minimal — seed `<FossilEditor/>` with the canonical `hello` example:

```tsx
import { helloExample } from '@fossil-lang/examples';
import { FossilEditor, NullTransport } from '@fossil-lang/editor';
import { useMemo } from 'react';

export function HelloDemo() {
  const transport = useMemo(() => new NullTransport(), []); // no LSP for the demo
  return <FossilEditor value={helloExample.mapping} lspTransport={transport} />;
}
```

Each `Example` carries `mapping` (the `.fossil` source), optional `csvw` (JSON-LD) + `shex` (target shape), and a `dataFiles[]` array with the referenced data files. The default resolver (`createDefaultResolver`) recognises `@examples/...` paths and serves bytes from the bundled `dataFiles` automatically.

### @fossil-lang/editor

> **NEW in v0.2.** No v0.1.x baseline; this is the canonical extraction path for in-tree `<CodeEditor/>` components.

Standalone Fossil editor — `<FossilEditor/>` + the four pluggable transports. Drop-in replacement for any in-tree CodeMirror + Fossil language + LSP wiring.

Public surface:

```typescript
import {
  FossilEditor,
  WorkerTransport,
  createWorkerTransport, // functional form, backwards-compat
  HttpTransport,
  NullTransport,
} from '@fossil-lang/editor';
import type {
  FossilEditorProps,
  Transport,
  HttpTransportOpts,
  WorkerTransportOpts,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcNotification,
  SendOptions,
} from '@fossil-lang/editor';
```

`FossilEditorProps` (load-bearing):

```typescript
interface FossilEditorProps {
  /** Controlled doc content. */
  value: string;
  /** Doc-change callback (debounced by CM6 update batching). */
  onChange?: (value: string) => void;
  /** Optional pre-composed CM6 extensions. When provided, the caller
   *  controls full composition (playground path). When omitted, the
   *  component auto-composes from lspTransport + resolver. */
  extensions?: Extension[];
  /** Pluggable LSP transport. `null` disables LSP entirely (syntactic
   *  highlighting + @-autocomplete still work via the language extension). */
  lspTransport: Transport | null;
  /** ConnectionResolver for @-prefix autocomplete. */
  resolver?: ConnectionResolver;
  /** Theming hook. Defaults to 'fossil-editor'. */
  className?: string;
}
```

**Three transports (one disable mode).** The choice depends on where your LSP runs — see the [Transport decision tree](#transport-decision-tree) for the picker. Quick reference:

| Transport         | Where LSP runs                                  | Cancellation |
| ----------------- | ----------------------------------------------- | ------------ |
| `WorkerTransport` | Web Worker in the browser (the playground way)  | n/a (Workers don't cancel postMessage) |
| `HttpTransport`   | Remote backend via JSON-RPC over POST (Keasy)   | AbortSignal-driven |
| `NullTransport`   | Nowhere — LSP features inert; highlighting only | n/a          |

Literal Keasy snippet — production HTTP-transport mount on a closed-source SaaS backend (commit `1672fe6`):

```typescript
const lspTransport = useMemo(
  () => new HttpTransport({ endpoint: "/v1/fossil/lsp" }),
  [],
);
// ...
<FossilEditor
  value={script}
  onChange={onScriptChange}
  lspTransport={lspTransport}
  resolver={resolver}
  className="flex-1"
/>
```

Cookie-session auth on the same origin handles credentials — no `Authorization` header required. The backend route the transport targets (`/v1/fossil/lsp` in Keasy) is a JSON-RPC adapter that dispatches to the per-org `AnalysisHost`; see [Keasy commit `1ae49d9`](#keasy-server-side-lsp-route) for the Rust route shape.

**Prose-mode** (no LSP at all — useful for free-form text editing in an editor wrapper):

```tsx
// assistant-wizard pattern — Keasy assistant-wizard.tsx via commit 1672fe6
<FossilEditor value={description} onChange={setDescription} lspTransport={null} />
```

`lspTransport={null}` is treated identically to `NullTransport` at the wiring level — the component skips `buildLspExtension()` and only composes the language + autocomplete extensions. The latter still requires a resolver if you want `@`-completion; if not, omit `resolver`.

### @fossil-lang/viewer

> **NEW in v0.2.** No v0.1.x baseline; this is the canonical extraction path for in-tree Cosmos.gl-based graph components.

Standalone graph viewer — `<FossilViewer/>` (IDE-style tabs: Graph + Turtle + Vertices + Edges) wrapping `<FossilGraphView/>` (the Cosmos.gl WebGL canvas + accessibility fallback). Promoted byte-equivalent from Keasy's `discovery/cosmos-graph.tsx` + `graph-view-v2.tsx`.

Public surface:

```typescript
import {
  FossilViewer,
  FossilGraphView,
  GraphCanvas,
  CosmosGraph,
  TabularFallback,
  TurtleTab,
  // Internals
  getAdaptiveConfig,
  DEFAULT_GRAPH_CONFIG,
  // Hooks
  useGraphData,
  useGraphCrossfilter,
  // Turtle adapter (free function)
  rowsToTurtle,
  // Constants
  GROUP_CSS_COLORS,
  hashPos,
  VIEWER_PACKAGE_VERSION,
} from '@fossil-lang/viewer';
import type {
  FossilViewerProps,
  FossilGraphViewProps,
  GraphCanvasProps,
  CosmosGraphProps,
  CosmosGraphHandle,
  TabularFallbackProps,
  TurtleTabProps,
  TurtleVertexRow,
  TurtleEdgeRow,
  VertexRow,
  EdgeRow,
  KGGraphData,
} from '@fossil-lang/viewer';
```

`FossilViewerProps` (the high-level tabs surface):

```typescript
interface FossilViewerProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
  selection?: Selection | null;    // Mosaic crossfilter (opt-in)
  onSelectVertex?: (v: VertexRow | null) => void;
  webgl?: boolean;                 // default true
  className?: string;
  prefixes?: Record<string, string>; // Turtle-tab IRI prefixes
  defaultTab?: 'graph' | 'turtle' | 'vertices' | 'edges';
}
```

`FossilGraphViewProps` (the canvas-only surface, no tabs):

```typescript
interface FossilGraphViewProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
  selection?: Selection | null;
  onSelectVertex?: (v: VertexRow | null) => void;
  webgl?: boolean;
  className?: string;
}
```

Rows are string-id-keyed. `VertexRow` has `id: string` + arbitrary other fields; `EdgeRow` has `source: string` + `target: string` + arbitrary other fields. If your data lives in a query coordinator (Mosaic, ArrowJS, your own), keep it there and materialize to this row shape in a thin adapter hook.

**`CosmosGraphHandle` API** — the imperative handle exposed via `graphRef`:

```typescript
interface CosmosGraphHandle {
  zoomIn(duration?: number): void;   // built-in 1.5× factor
  zoomOut(duration?: number): void;  // built-in 1.5× factor
  fitView(duration?: number): void;
  recenter(): void;
  // ... + lifecycle methods
}
```

**Deliberate API contract change in v0.2:** the v0.1.x-era raw `zoom(factor, duration)` method was removed during the Phase 12 viewer port. Use `zoomIn(duration)` / `zoomOut(duration)` instead — the 1.5× factor is built in. See [Troubleshooting](#my-fossilviewer-floating-controls-broke-after-upgrade).

Literal Keasy snippet — the 3-consumer swap from `commit 7e8146c`:

```typescript
// Before — three pages imported from in-tree duplicates:
//   import { GraphCanvas, DEFAULT_GRAPH_CONFIG } from "@/components/discovery/graph-view-v2";
//   import type { CosmosGraphHandle } from "@/components/discovery/cosmos-graph";

// After — single import path:
import {
  GraphCanvas,
  DEFAULT_GRAPH_CONFIG,
  type CosmosGraphHandle,
} from "@fossil-lang/viewer";
```

Synthetic minimal — 4 vertices + 3 edges rendered in the IDE-tab shell:

```tsx
import { FossilViewer } from '@fossil-lang/viewer';

const vertices = [
  { id: 'alice', type: 'Person', label: 'Alice' },
  { id: 'bob',   type: 'Person', label: 'Bob' },
  { id: 'carol', type: 'Person', label: 'Carol' },
  { id: 'dave',  type: 'Person', label: 'Dave' },
];
const edges = [
  { source: 'alice', target: 'bob',   label: 'knows' },
  { source: 'bob',   target: 'carol', label: 'knows' },
  { source: 'carol', target: 'dave',  label: 'knows' },
];

export const GraphDemo = () => <FossilViewer vertices={vertices} edges={edges} />;
```

If your runtime has no WebGL2 (older corporate browser policies, headless test envs without GPU emulation), pass `webgl={false}` to force the `<TabularFallback/>` accessible-tables view. The viewer also auto-detects WebGL2 availability at mount and falls back on failure.

### @fossil-lang/playground

> **Composition root, not a primitive.** Use this if you want the full 5-tab IDE experience (Mapping / Source / Shape + Output / Compiled SQL); use `@fossil-lang/editor` + `@fossil-lang/viewer` directly if you want the parts.

Public surface (high-level):

```typescript
import {
  FossilPlayground,
  // Re-exports for v0.1.x backwards compatibility (single module instance):
  FossilEditor,           // from @fossil-lang/editor
  FossilGraphView,        // from @fossil-lang/viewer
  FossilViewer,           // from @fossil-lang/viewer
  ResultGraph,            // v0.2.x deprecated alias of FossilGraphView (lazy-loaded)
  ResultTable,
} from '@fossil-lang/playground';
import type {
  FossilPlaygroundProps,
  VertexRow, EdgeRow,
  ResultGraphProps, ResultTableProps,
} from '@fossil-lang/playground';
```

`FossilPlaygroundProps` (load-bearing — abbreviated):

```typescript
interface FossilPlaygroundProps {
  resolver: ConnectionResolver;          // REQUIRED (CONN-01)
  wasmUrl: string | URL;                 // REQUIRED
  workerUrl?: string | URL;              // override (tests typically)
  initialMapping?: string;
  maxResolvedBytes?: number;             // default 10MB
  onRun?: (result: { vertices: VertexRow[]; edges: EdgeRow[] }) => void;
  onError?: (error: Error) => void;
  theme?: FossilThemeProp;               // 'light' | 'dark' | FossilTheme | undefined
  initialPermalink?: string;
  onStateChange?: (permalink: string) => void;
}
```

The `theme` prop is host-provider-aware (per ADR-0035). When omitted, the component injects NO `--fossil-*` CSS vars — the host is expected to supply the cascade via an ancestor theme provider (e.g. `<KanzoThemeProvider/>` from `@kanzo/theme`). v0.1.x consumers passing `'light'` or `'dark'` explicitly see identical behaviour.

Synthetic minimal mount (the same 3-line invocation from [Quickstart](#quickstart), expanded with a permalink hook):

```tsx
import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';
import { useMemo } from 'react';

export function Embedded() {
  const resolver = useMemo(() => createDefaultResolver(), []);
  return (
    <FossilPlayground
      resolver={resolver}
      wasmUrl={wasmUrl}
      onStateChange={(perma) => window.history.replaceState(null, '', '#' + perma)}
      onError={(err) => console.error('[fossil]', err)}
    />
  );
}
```

The component does NOT touch `window.location` itself — the host pipes the encoded permalink into `history.replaceState` (or your router of choice). This separation lets the playground embed inside Next.js, Remix, plain SPAs, or test harnesses without coupling to a routing model.

## Transport decision tree

`<FossilEditor/>` accepts any object satisfying the `Transport` interface (per ADR-0036 — superset of `@codemirror/lsp-client`'s `Transport` shape, with cancellation + lifecycle). The three built-ins cover the three deployment patterns; pick by walking the questions below.

```
Q1: Does your app already have a backend that can host an LSP-like route?

  YES → HttpTransport
    Use when: closed-source SaaS, products with auth gates, multi-tenant
              backends where the Rust AnalysisHost runs server-side per
              org/tenant. This is the Keasy pattern.

    Example (~10 LOC):

      import { FossilEditor, HttpTransport } from '@fossil-lang/editor';
      import { useMemo } from 'react';

      function ServerLspEditor({ value, onChange }) {
        const transport = useMemo(
          () => new HttpTransport({ endpoint: '/api/fossil/lsp' }),
          [],
        );
        return <FossilEditor value={value} onChange={onChange}
                             lspTransport={transport} />;
      }

    Backend reference: Keasy commit `1ae49d9` — POST /v1/fossil/lsp accepts
    JsonRpcRequest, dispatches to the per-org AnalysisHost, piggy-backs
    publishDiagnostics in didChange responses. ~420 LOC of Rust.

  NO → continue to Q2

Q2: Can you run a Web Worker in the host (browser-only deploy)?

  YES → WorkerTransport
    Use when: pure browser app, no server backend, you want the LSP to
              run client-side via fossil-wasm in a Worker scope. This is
              the playground pattern (apps/landing/).

    Example (~12 LOC):

      import { FossilEditor, WorkerTransport } from '@fossil-lang/editor';
      import { useMemo, useEffect } from 'react';

      function WorkerLspEditor({ value, onChange }) {
        const transport = useMemo(() => {
          const worker = new Worker(new URL('./lsp.worker.ts', import.meta.url),
                                    { type: 'module' });
          worker.postMessage({ __boot: true, wasmUrl: '/fossil_wasm_bg.wasm' });
          return new WorkerTransport({ worker });
        }, []);
        useEffect(() => () => transport.close(), [transport]);
        return <FossilEditor value={value} onChange={onChange}
                             lspTransport={transport} />;
      }

    Lifecycle note: the playground's <FossilPlayground/> handles Worker
    boot + termination internally. You only manage the Worker lifecycle
    when consuming <FossilEditor/> directly (per ADR-0026).

  NO → continue to Q3

Q3: Do you want LSP features (hover, diagnostics, completion) at all?

  YES, but no Worker available (SSR, test env, edge runtime) →
        currently no built-in fits.

    The Transport interface is exported — you can write a custom one
    (in-process AnalysisHost, mock, recorded fixtures for tests). The
    package was originally designed with a third "Direct" transport in
    mind (in-process); for v0.2 this remains an extension point rather
    than a built-in.

  NO  → NullTransport (or lspTransport={null})
    Use when: free-form prose editing, read-only static editor, demo
              previews. Syntactic highlighting + @-autocomplete (via
              ConnectionResolver) still work; only LSP-driven features
              (hover, diagnostics, semantic-tokens overlay) are inert.

    Example (~6 LOC):

      import { FossilEditor } from '@fossil-lang/editor';

      export const Preview = ({ source }: { source: string }) =>
        <FossilEditor value={source} lspTransport={null} />;

    This is the assistant-wizard StepDescribe pattern from Keasy commit
    `1672fe6` — free-form description prose, no language extension, no
    LSP traffic.
```

**Custom transports.** The `Transport` interface is intentionally narrow (`send` + `subscribe` + `unsubscribe` + optional `close`). If none of the three built-ins fit, implement your own — see `packages/editor/src/transports/Null.ts` (~26 LOC) for the minimal shape. Common reasons to implement custom: in-process testing (synchronous, deterministic), recorded LSP fixtures, batched/multiplexed HTTP, or alternative wire formats (WebSocket subscriptions, gRPC-Web).

## CSS theming integration

The `@fossil-lang/*` packages are theme-less by default (per ADR-0035 — visual ownership separation). When mounted with `theme={undefined}`, the components inject NO `--fossil-*` CSS variables at the root; the host supplies the cascade.

**Token vocabulary (per ADR-0034).** Components read CSS custom properties under the `--fossil-*` namespace, mechanical-flat-named:

```css
--fossil-colors-background
--fossil-colors-foreground
--fossil-colors-border
--fossil-colors-ring
--fossil-fonts-sans
--fossil-fonts-mono
--fossil-fonts-sizeBase
--fossil-radii-md
--fossil-spacing-2
--fossil-motion-duration-fast
/* ... and ~20 more */
```

Two integration patterns:

**Pattern A — `<KanzoThemeProvider/>` (for kanzo-branded hosts).** Wrap your subtree in the brand-owned theme provider from `@kanzo/theme`:

```tsx
import { KanzoThemeProvider } from '@kanzo/theme';
import { FossilPlayground } from '@fossil-lang/playground';

<KanzoThemeProvider>
  <FossilPlayground resolver={resolver} wasmUrl={wasmUrl} />
</KanzoThemeProvider>
```

**Pattern B — host-side CSS alias bridge (for hosts with their own design system, e.g. shadcn/Tailwind, MUI, Chakra).** If your host already defines a token vocabulary (`--background`, `--foreground`, `--ring`, ...), alias the `--fossil-*` names to your existing tokens. This is the Keasy pattern.

Literal Keasy snippet — `web/src/app/globals.css` (Keasy commit `305b6c6`, MIG-04 in Phase 16 plan 16-02; 26 names surveyed from the published packages' actual consumption):

```css
:root {
  /* ...existing shadcn tokens... */

  /* === @fossil-lang/* CSS-var bridge (MIG-04) === */
  /* Maps the 26 --fossil-* names consumed by @fossil-lang/{ui,viewer,editor,codemirror-fossil}
     to Keasy's shadcn token vocabulary + literal defaults mirroring @kanzo/theme.
     Cascade-based dark-mode coverage via var() indirection — no .dark redeclarations needed. */

  /* Colors (9) — alias to shadcn vocab */
  --fossil-colors-background: var(--background);
  --fossil-colors-foreground: var(--foreground);
  --fossil-colors-muted:      var(--muted-foreground);
  --fossil-colors-border:     var(--border);
  --fossil-colors-accent:     var(--primary);
  --fossil-colors-ring:       var(--ring);
  --fossil-colors-error:      var(--destructive);
  --fossil-colors-warning:    #f59e0b;
  --fossil-colors-info:       #3b82f6;

  /* Fonts (3) */
  --fossil-fonts-sans: var(--font-sans, system-ui);
  --fossil-fonts-mono: var(--font-mono, ui-monospace);
  --fossil-fonts-sizeBase: 13px;
  /* ... 18 more aliases for radii / spacing / motion / size ... */
}

.dark {
  /* Cascade-based dark-mode coverage: the --fossil-* aliases above resolve
     through var(--background) etc., which Keasy already swaps under .dark.
     No --fossil-* redeclarations needed. */
}
```

**Survey-before-bridge.** The 26 names committed by Keasy were grep-discovered, not ADR-listed. The plan's anticipated list disagreed with the actual published-package consumption (the plan used `--fossil-colors-bg|fg` short forms; the packages consume `--fossil-colors-background|foreground` full forms). Always grep what the packages actually read before authoring your bridge:

```bash
grep -rhEo '--fossil-[a-zA-Z0-9-]+' node_modules/@fossil-lang/{editor,viewer,ui,codemirror-fossil}/dist/ \
  | sort -u
```

Synthetic minimal — a non-Keasy host with its own design system:

```css
:root {
  /* Your existing tokens */
  --my-bg: #fafafa;
  --my-fg: #0f172a;
  --my-border: #e2e8f0;

  /* Bridge the @fossil-lang/* names to yours */
  --fossil-colors-background: var(--my-bg);
  --fossil-colors-foreground: var(--my-fg);
  --fossil-colors-border:     var(--my-border);
}
```

That's the entire integration — no JS, no React, no new packages. The `<FossilEditor/>` + `<FossilViewer/>` you mount inherit your palette via pure cascade.

## Troubleshooting

### Why was there a `pnpm.overrides` block in some early adopter setups?

If you're vendoring pre-publish tarballs (`pnpm pack` + `file:` protocol — the Phase 16 mechanism, ADR-0038), the tarball's `package.json` declares its workspace deps as plain version pins (e.g. `"@fossil-lang/types": "^0.1.0"`) — pins that `pnpm` will try to fetch from the registry, where they don't yet exist. The `pnpm.overrides` block redirects those transitive specifiers to your local tarballs.

Literal Keasy block (commit `86056aa`, `feat(fossil-16-03): viewer adapter + pnpm.overrides for file: deps`):

```json
{
  "pnpm": {
    "overrides": {
      "@fossil-lang/codemirror-fossil": "file:./vendor/fossil-lang/fossil-lang-codemirror-fossil-0.1.0.tgz",
      "@fossil-lang/editor":            "file:./vendor/fossil-lang/fossil-lang-editor-0.1.0.tgz",
      "@fossil-lang/resolvers":         "file:./vendor/fossil-lang/fossil-lang-resolvers-0.1.0.tgz",
      "@fossil-lang/types":             "file:./vendor/fossil-lang/fossil-lang-types-0.1.0.tgz",
      "@fossil-lang/ui":                "file:./vendor/fossil-lang/fossil-lang-ui-0.1.0.tgz",
      "@fossil-lang/viewer":            "file:./vendor/fossil-lang/fossil-lang-viewer-0.1.0.tgz",
      "@fossil-lang/wasm":              "file:./vendor/fossil-lang/fossil-lang-wasm-0.1.0.tgz",
      "@kanzo/theme":                   "file:./vendor/fossil-lang/kanzo-theme-0.1.0.tgz"
    }
  }
}
```

**Once you flip to registry deps (`"@fossil-lang/editor": "^0.2.0"`), the overrides block becomes unnecessary and should be deleted in the same commit.** See ADR-0038 §Compliance (`decisions/0038-cross-repo-consumption.md`) for the canonical lifecycle: the `file:` indirection + overrides are a Phase 16 (pre-publish) mechanism, removed at Phase 17 REL-01 (registry publish) close.

For published v0.2.0+, you do NOT need this block. If you copied a `pnpm.overrides` block from a Phase 16-era setup, delete it after updating your `dependencies` to `^0.2.0`.

### My `<FossilViewer/>` floating-controls broke after upgrade

If your code calls `graphRef.current.zoom(factor, duration)` and you see a TypeScript error like:

```
Property 'zoom' does not exist on type 'CosmosGraphHandle'.
Did you mean 'zoomIn' or 'zoomOut'?
```

— that's the deliberate Phase 12 API contract change. The raw `zoom(factor, duration)` method was removed in favor of purpose-named `zoomIn(duration)` / `zoomOut(duration)` (the 1.5× factor is built in). The motivation: the raw method leaked cosmos.gl internals into the public viewer surface; the purpose-named methods are more discoverable and don't require callers to remember the canonical zoom factors.

Before (Keasy `floating-controls.tsx`, v0.1.x in-tree):

```typescript
{ key: "in",  icon: Plus,  label: "Zoom in",  action: () => graphRef.current?.zoom(1.5, 300) },
{ key: "out", icon: Minus, label: "Zoom out", action: () => graphRef.current?.zoom(0.5, 300) },
```

After (Keasy commit `7e8146c`):

```typescript
{ key: "in",  icon: Plus,  label: "Zoom in",  action: () => graphRef.current?.zoomIn(300) },
{ key: "out", icon: Minus, label: "Zoom out", action: () => graphRef.current?.zoomOut(300) },
```

`fitView(duration)` is unchanged; `recenter()` is unchanged. Only `zoom()` was removed.

### `tsc` surfaced unexpected import errors after deletion

After deleting in-tree components, `tsc --noEmit` may report import errors in sibling files you didn't think were consumers. This happens because **alias-pattern grep doesn't catch relative imports between files in the same directory.**

Phase 16 plan 16-03 hit this exactly: the plan's grep used `@/components/discovery/...` (the TypeScript path alias), which correctly identified 3 consumers. After deleting the in-tree files, `tsc --noEmit` reported errors in two additional files: `floating-controls.tsx` and `graph-settings.tsx`. Both lived in the same directory as the deleted files, so they imported via `./cosmos-graph` (relative) rather than `@/components/discovery/cosmos-graph` (alias) — invisible to the alias-only grep.

**Recipe.** Use a wider grep that includes the relative-sibling pattern, AND always run `tsc --noEmit` post-deletion as the source of truth:

```bash
# Wider grep — catches sibling imports
grep -rln 'discovery/cosmos-graph\|discovery/graph-view\|\./cosmos-graph\|\./graph-view' src/

# Source of truth
pnpm exec tsc --noEmit
```

The Phase 16 fixups were mechanical — each was a one-line redirect from the relative path to `@fossil-lang/viewer`. Treat any tsc errors directly caused by your deletion as in-scope under Rule 3 (Blocking) and fix inline; don't defer.

### `<FossilEditor/>` hover popup returns nothing

If the editor renders, syntactic highlighting works, `@`-autocomplete works, but hovering over a type name produces no popup — that's a known graceful-degradation point. The upstream `AnalysisHost.hover` API isn't yet wired (a future minor of the published packages will add it; tracked in Phase 16 SUMMARY's "Verification gaps" section).

The editor degrades silently:
- No popup appears
- No error in the console
- No log spam in the network panel

The CM6 `LSPClient` tolerates `null` results on every probe — syntax highlighting and `@`-autocomplete (driven by your `ConnectionResolver`, not LSP) continue to work normally.

Same disposition for semantic tokens. The route arm returns `null`; the editor falls back to the syntactic-highlighting palette from `@fossil-lang/codemirror-fossil`, which is sufficient for almost all reading.

**Workaround:** none needed — degradation is intentional. **Permanent fix:** ships with the upstream `AnalysisHost` surface expansion (target: next minor after v0.2.0).

### Next.js `next build` fails on env-var prereqs

If your host is a Next.js app and `next build` fails with missing-env-var errors, this is a host-side concern, not a `@fossil-lang/*` issue. Phase 16's verification ran `pnpm exec tsc --noEmit` + `pnpm test` as the binding signals for migration correctness; `next build` was deferred to operators with the right env-var setup. The migration touches no Next.js-specific code paths.

If you want to gate your migration PR on a `next build` smoke, mirror Keasy's pattern: run `tsc + vitest` in CI (cheap, no secrets) and run `next build` locally with your dev env vars before merging.

## Live demos + further reading

- **Playground (5-tab demo, all features):** <https://playground.kanzo.dev> (source: `apps/landing/`).
- **Multi-host fixture (embedded-in-host-shell demo):** <https://playground.kanzo.dev/multi-host> (source: `apps/landing/app/multi-host/`, NEW in v0.2 — built in plan 17-04 as a Keasy-styled host shell wrapping `<FossilEditor/>` + `<FossilViewer/>`; demonstrates the embedded-in-another-host case).
- **ADR-0038 (cross-repo consumption pattern):** [`decisions/0038-cross-repo-consumption.md`](decisions/0038-cross-repo-consumption.md) — the `pnpm pack` + `file:` mechanism Phase 16 used; §Compliance documents removal at v0.2.0 publish.
- **ADR-0034 (mechanical-flatten token vocabulary):** [`decisions/0034-css-variable-naming-mechanical-flatten.md`](decisions/0034-css-variable-naming-mechanical-flatten.md) — the `--fossil-namespace-key` naming convention.
- **ADR-0035 (visual ownership separation):** [`decisions/0035-visual-ownership-separation.md`](decisions/0035-visual-ownership-separation.md) — why `@fossil-lang/*` is theme-less by default.
- **ADR-0036 (Transport-superset design):** [`decisions/0036-transport-superset.md`](decisions/0036-transport-superset.md) — why the `Transport` interface adds `SendOptions.signal` + `close?()` over CM6's narrower contract.
- **Per-package READMEs:** each of the 8 packages ships its own README in `packages/{playground,codemirror-fossil,wasm,types,resolvers,examples,editor,viewer}/README.md`. The READMEs cover full API surfaces; this guide covers consumption patterns + migration recipes.
- **Keasy migration as a case study:** Phase 16 plans 16-01..16-05 — see [`.planning/phases/16-keasy-migration/16-SUMMARY.md`](.planning/phases/16-keasy-migration/16-SUMMARY.md) for the cross-cutting narrative (707 LOC deleted in Keasy + new `/v1/fossil/lsp` JSON-RPC route + CSS bridge + multi-host pattern proven against a real product).
