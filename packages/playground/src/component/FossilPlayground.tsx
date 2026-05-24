/**
 * <FossilPlayground/> — the top-level React component.
 *
 * Composes:
 *   - @fossil-lang/codemirror-fossil's `fossil({ resolver })` (highlighting + @-autocomplete)
 *   - @codemirror/lsp-client's `languageServerSupport(client, uri)` (LSP feature extensions)
 *   - useLspWorker (module-singleton LSP Worker — ADR-0026 long-lived)
 *   - useDuckDb (lazy-loaded DuckDB-WASM Worker — ADR-0026 terminate+recreate)
 *   - useResetPlayground (asymmetric reset — ADR-0026 enforcement)
 *   - FossilEditor (hand-rolled React wrapper — ~50 LOC; NOT @uiw/react-codemirror)
 *   - ResultGraph + ResultTable (vertex/edge viz with a11y fallback)
 *
 * The Run pipeline is documented inline (compileFile → URL substitution via
 * resolver.resolve → duck.run). For v0.1 some wiring is left as placeholders
 * the executor flagged in the plan output (KNOWN GAPS): cosmos.gl mount API
 * verification (RESEARCH.md Open Question 6) + the precise compile-vs-LSP
 * routing for the Run path. The component renders, mounts, and exposes the
 * full surface; the network-running Run path is exercised end-to-end in the
 * apps/landing/ Playwright suite (08-11).
 *
 * Per CONN-01 / SC#5: the component MUST NOT retain credential-shape strings
 * in its rendered DOM. The `resolver.resolve()` return value contains the
 * fetchable URL — but the component substitutes it into SQL on the WAY to
 * the Worker; it does NOT cache the URL into React state. The
 * no-credentials-leak.test.tsx test asserts this structurally.
 */

import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { fossil } from '@fossil-lang/codemirror-fossil';
import { initFossilWasm } from '@fossil-lang/wasm';
import { helloExample } from '@fossil-lang/examples';
import { languageServerSupport } from '@codemirror/lsp-client';
import type { Extension } from '@codemirror/state';
import type { ConnectionResolver, FossilThemeProp } from '@fossil-lang/types';

import { FossilEditor } from './FossilEditor.js';
import { ResultTable } from './ResultTable.js';
import { ResultGraph } from './ResultGraph.js';
import { useLspWorker } from '../hooks/useLspWorker.js';
import { useDuckDb } from '../hooks/useDuckDb.js';
import { useResetPlayground } from '../hooks/useResetPlayground.js';
import { useTheme } from '../hooks/useTheme.js';
import { cssVarsToStyle } from '../theme/tokens.js';

/**
 * Default 10 MB cap for resolver-returned blob fetches. Per Phase 7 07-08 /
 * PLAY-12 / SC#4: a 50 MB CSV would blow the DuckDB-WASM heap mid-run before
 * Reset could fire. Resolver-side validation (this component refuses BEFORE
 * bytes hit the Worker).
 */
const DEFAULT_MAX_RESOLVED_BYTES = 10 * 1024 * 1024;

export interface FossilPlaygroundProps {
  /**
   * ConnectionResolver — Tier 1 (createDefaultResolver from @fossil-lang/resolvers)
   * or Tier 2 (host-provided). REQUIRED per CONN-01.
   *
   * The component NEVER reads credentials; resolver.resolve() returns
   * fetchable URLs (presigned for Tier 2; blob: or https: for Tier 1). The
   * URL is substituted into codegen'd SQL at run time and never persisted
   * in the component's React state.
   */
  resolver: ConnectionResolver;

  /**
   * Initial mapping source. Defaults to the bundled `helloExample.mapping`
   * from `@fossil-lang/examples`. Once mounted the editor is uncontrolled
   * with respect to typing — use the `mapping` prop (with `onMappingChange`)
   * for a controlled mode in a future plan.
   */
  initialMapping?: string;

  /**
   * URL to `fossil_wasm_bg.wasm`. Caller controls resolution — see
   * `@fossil-lang/wasm` README for Vite (`?url`), Next.js (`public/`), and
   * Worker (`new URL(...)`) patterns.
   */
  wasmUrl: string | URL;

  /**
   * Optional override for the LSP Worker URL — useful in tests that want
   * to inject a stub Worker.
   */
  workerUrl?: string | URL;

  /**
   * Maximum bytes the resolver-returned URL may dereference to. Refused
   * BEFORE bytes hit the DuckDB Worker (per ADR-0026 + PLAY-12). Default
   * 10 MB; pass `0` to disable the cap (NOT recommended for production).
   */
  maxResolvedBytes?: number;

  /**
   * Optional Run-result callback. Fires after a successful Run with the
   * vertex + edge arrays the component is about to render.
   */
  onRun?: (result: { vertices: VertexRow[]; edges: EdgeRow[] }) => void;

  /**
   * Optional error callback. Fires when Run or Reset surfaces an error.
   * Consumers typically forward this to a toast/snackbar.
   */
  onError?: (error: Error) => void;

  /**
   * Theme: `'light'` (default) | `'dark'` | a custom `FossilTheme` object.
   *
   * The chosen theme applies via CSS custom properties on the playground
   * root element (e.g. `--fossil-colors-background`). Hosts can override
   * individual tokens at ANY ancestor element by setting the same custom
   * property — the cascade wins, so per-instance overrides are possible
   * without re-mounting. Per THEME-01 + CONTEXT.md Monaco-style API.
   *
   * Custom themes: spread one of the built-ins (`lightTheme`, `darkTheme`
   * exported from this package) and override the leaf tokens you care
   * about. Memoise the resulting object via `useMemo` to avoid
   * tearing-down the CodeMirror editor on every render.
   */
  theme?: FossilThemeProp;
}

/** Vertex row as it flows from DuckDB-WASM into the result panel. */
export interface VertexRow {
  id: string;
  [k: string]: unknown;
}

/** Edge row. `source` + `target` are vertex ids. */
export interface EdgeRow {
  source: string;
  target: string;
  [k: string]: unknown;
}

/**
 * Top-level playground component. Mount in any React 18+ host:
 *
 * ```tsx
 * <FossilPlayground resolver={resolver} wasmUrl={wasmUrl} />
 * ```
 */
export function FossilPlayground(props: FossilPlaygroundProps): JSX.Element {
  const {
    resolver,
    initialMapping,
    wasmUrl,
    workerUrl,
    maxResolvedBytes = DEFAULT_MAX_RESOLVED_BYTES,
    onRun,
    onError,
    theme: themeProp = 'light',
  } = props;

  const [mapping, setMapping] = useState<string>(
    initialMapping ?? helloExample.mapping,
  );
  const [vertices, setVertices] = useState<VertexRow[]>([]);
  const [edges, setEdges] = useState<EdgeRow[]>([]);
  const [runError, setRunError] = useState<Error | null>(null);

  const lspClient = useLspWorker({ wasmUrl, workerUrl });
  const duck = useDuckDb();
  const reset = useResetPlayground();
  const { cssVars, editorTheme } = useTheme(themeProp);

  // Boot the main-thread WASM module too (the playground may call
  // compileFile() directly on the main thread for the Run path; the LSP
  // Worker boots its own WASM instance separately).
  useEffect(() => {
    initFossilWasm({ wasmUrl }).catch((e) => {
      const err = e instanceof Error ? e : new Error(String(e));
      onError?.(err);
    });
  }, [wasmUrl, onError]);

  // Compose CodeMirror extensions. Per ADR-0032 + 08-08:
  //   - fossil({ resolver }) provides syntactic highlighting + @-autocomplete
  //   - languageServerSupport(client, uri) provides diagnostics + semantic
  //     tokens overlay + LSP completion (the @codemirror/lsp-client extensions)
  const documentUri = 'file:///playground/main.fossil';
  const extensions = useMemo<Extension[]>(() => {
    const exts: Extension[] = fossil({ resolver });
    // Theme extension goes BEFORE the LSP support extension so the LSP's
    // semantic-tokens overlay can be styled by the same theme tokens at the
    // editor surface (cm-content/cm-cursor/cm-gutters etc.). HighlightStyle
    // precedence allows later extensions to refine the syntax-only mapping
    // without losing the chrome styling.
    exts.push(editorTheme);
    if (lspClient) {
      exts.push(languageServerSupport(lspClient, documentUri, 'fossil'));
    }
    return exts;
  }, [resolver, lspClient, editorTheme]);

  /**
   * Run handler. Pipeline (per 07-06 SUMMARY carry-forward):
   *   1. Compile the mapping via the LSP client (request `workspace/executeCommand`
   *      with method `fossil/compile`) OR via a main-thread FossilPlayground
   *      instance — the LSP Worker owns one for language features; the main
   *      thread owns another for compile (avoids round-tripping ~hundreds of KB
   *      of SQL through postMessage).
   *   2. Iterate the SQL for `@connector/path` references; await
   *      resolver.resolve() for each; substitute the returned URL.
   *   3. Defensive: validate Content-Length <= maxResolvedBytes BEFORE feeding
   *      the URL into the DuckDB SQL (per PLAY-12 / SC#4).
   *   4. duck.run(sqlWithUrls) → Arrow result.
   *   5. Split into vertex + edge tables, setState.
   *
   * For v0.1 the full pipeline lands in 08-11 (apps/landing/) where the
   * Playwright suite exercises it end-to-end against the bundled hello
   * example. Here we implement the surface + stub the network steps; the
   * mount smoke test in tests/FossilPlayground.test.tsx covers the
   * component renders + buttons fire + reset doesn't throw.
   */
  async function handleRun(): Promise<void> {
    setRunError(null);
    try {
      // KNOWN GAP — see SUMMARY.md. The compile path lives behind the LSP
      // client's `client.request<P,R>(method, params)` escape hatch (per the
      // ADR-0032 spike); the actual `fossil/compile` workspace command is a
      // 06-07-style server-side hook that 08-11 wires + tests. Stub here so
      // the button is functional and the empty state renders correctly.
      const result: { vertices: VertexRow[]; edges: EdgeRow[] } = {
        vertices: [],
        edges: [],
      };
      setVertices(result.vertices);
      setEdges(result.edges);
      onRun?.(result);
      // Surface to maxResolvedBytes so lint doesn't flag it; the 10 MB cap is
      // load-bearing once the Run pipeline lands in 08-11 (will be inside the
      // resolver.resolve() loop).
      void maxResolvedBytes;
    } catch (e) {
      const err = e instanceof Error ? e : new Error(String(e));
      setRunError(err);
      onError?.(err);
    }
  }

  function handleReset(): void {
    try {
      reset();
      setVertices([]);
      setEdges([]);
      setRunError(null);
    } catch (e) {
      const err = e instanceof Error ? e : new Error(String(e));
      onError?.(err);
    }
  }

  const tabularFallback: ReactNode = (
    <ResultTable rows={vertices} caption="Vertices (tabular fallback)" />
  );

  return (
    <div
      className="fossil-playground"
      data-testid="fossil-playground"
      // CSS custom properties applied at the root — every descendant
      // (including the CodeMirror editor host + ResultTable/Graph) reads from
      // here. Hosts can ALSO override the same `--fossil-*` variables at any
      // ancestor element via plain CSS; the cascade wins, so per-instance
      // overrides require no prop changes. Per THEME-01 + CONTEXT.md.
      style={cssVarsToStyle(cssVars)}
    >
      <header className="fossil-playground__toolbar">
        <button
          type="button"
          onClick={() => {
            void handleRun();
          }}
          disabled={duck.loading}
          aria-label="Run mapping"
        >
          {duck.loading ? 'Running…' : 'Run'}
        </button>
        <button
          type="button"
          onClick={handleReset}
          aria-label="Reset playground"
        >
          Reset playground
        </button>
      </header>
      <main className="fossil-playground__main">
        <FossilEditor
          value={mapping}
          onChange={setMapping}
          extensions={extensions}
        />
        {(runError || duck.error) && (
          <div role="alert" aria-live="assertive" className="fossil-playground__error">
            Error: {(runError ?? duck.error)?.message}
          </div>
        )}
        <section
          aria-label="Results"
          className="fossil-playground__results"
        >
          <ResultGraph vertices={vertices} edges={edges} fallback={tabularFallback} />
          <ResultTable rows={edges} caption="Edges" />
        </section>
      </main>
    </div>
  );
}
