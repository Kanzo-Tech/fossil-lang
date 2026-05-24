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
 * The Run pipeline (compileFile → URL substitution via resolver.resolve →
 * COPY-rewrite → DuckDB execute → vertex/edge projection) lives in
 * `../run/runPipeline.ts` so it can be reused by advanced consumers building
 * custom layouts (the same surface 08-09 SUMMARY anticipated).
 *
 * Per ADR-0026 there are now THREE WASM-side resources with three distinct
 * lifecycles:
 *   - LSP Worker (module-singleton, long-lived; never terminated by Reset)
 *   - DuckDB Worker (module-singleton, terminate+recreate on Reset)
 *   - main-thread `FossilPlayground` (component-scope, `free()` on unmount
 *     AND on Reset so Salsa state doesn't compound across many resets)
 *
 * Per CONN-01 / SC#5: the component MUST NOT retain credential-shape strings
 * in its rendered DOM. The `resolver.resolve()` return value contains the
 * fetchable URL — but the pipeline substitutes it into SQL on the WAY to
 * the Worker; it does NOT cache the URL into React state. The
 * no-credentials-leak.test.tsx test asserts this structurally.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { fossil } from '@fossil-lang/codemirror-fossil';
import {
  initFossilWasm,
  FossilPlayground as FossilPlaygroundWasm,
  type FileHandle,
} from '@fossil-lang/wasm';
import { helloExample } from '@fossil-lang/examples';
import { languageServerSupport } from '@codemirror/lsp-client';
import type { Extension } from '@codemirror/state';
import type { ConnectionResolver, FossilThemeProp } from '@fossil-lang/types';

import { FossilEditor } from './FossilEditor.js';
import { ResultTable } from './ResultTable.js';
import { ResultGraph } from './ResultGraph.js';
import { useLspWorker } from '../hooks/useLspWorker.js';
import { useDuckDb, getDuckDb } from '../hooks/useDuckDb.js';
import { useResetPlayground } from '../hooks/useResetPlayground.js';
import { useTheme } from '../hooks/useTheme.js';
import { cssVarsToStyle } from '../theme/tokens.js';
import { announce, ARIA_LABELS } from '../a11y/index.js';
import { runPipeline } from '../run/runPipeline.js';

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
  // Main-thread WASM init gate. CodeMirror's StreamParser eagerly calls
  // tokenize() on every line at editor-mount time; if WASM hasn't
  // initialised yet the call hits a __wbindgen_malloc_command_export
  // undefined property and crashes the editor render. We defer the
  // <FossilEditor/> mount until initFossilWasm resolves so the parser
  // sees a ready WASM module on its first invocation.
  //
  // Surfaced by the 08-11 Playwright suite (the unit tests stub
  // tokenize via vi.mock so the race never fires in vitest).
  const [wasmReady, setWasmReady] = useState<boolean>(false);

  const lspClient = useLspWorker({ wasmUrl, workerUrl });
  const duck = useDuckDb();
  const reset = useResetPlayground();
  const { cssVars, editorTheme } = useTheme(themeProp);

  /**
   * Main-thread `FossilPlayground` instance + opened file handle.
   *
   * Why component-scope (not module-singleton like the LSP Worker):
   *   - The LSP Worker is the long-lived language-features instance per
   *     ADR-0026; it owns its own Salsa store inside the Worker's WASM
   *     module. We need a SEPARATE main-thread instance for the Run path
   *     so we don't postMessage hundreds of KB of generated SQL through
   *     the Worker boundary on every Run.
   *   - The main-thread instance must `free()` on unmount so the Salsa
   *     store + interned strings inside the WASM linear memory don't
   *     leak when a host re-mounts the playground (e.g. an examples
   *     gallery switching between mappings in Phase 9).
   *   - It must ALSO `free()` on Reset so Salsa state doesn't compound
   *     across many resets (mirrors the DuckDB Worker terminate+recreate
   *     spirit — recreating the compile instance is cheap, ~hundreds of
   *     μs, vs the ~hundreds of ms DuckDB-WASM cold-start).
   *
   * The handle is lazy: first Run mints the instance + openFile, every
   * subsequent Run updateFiles the same handle so Salsa benefits from
   * incremental memoisation (ADR-0022 — set_text bumps the revision,
   * downstream tracked queries invalidate incrementally).
   */
  const compileInstanceRef = useRef<{
    instance: FossilPlaygroundWasm | null;
    handle: FileHandle | null;
  }>({ instance: null, handle: null });

  /** Stable URI the LSP client + the main-thread compile instance both key on. */
  const documentUri = 'file:///playground/main.fossil';

  /**
   * Free the component-scope main-thread FossilPlayground instance. Idempotent
   * so it's safe to call from both the unmount cleanup and from handleReset.
   * The LSP Worker is untouched (ADR-0026 asymmetric API).
   */
  const freeCompileInstance = useCallback((): void => {
    const r = compileInstanceRef.current;
    if (r.instance) {
      try {
        r.instance.free();
      } catch {
        // free() on an already-freed instance throws — swallow.
      }
    }
    compileInstanceRef.current = { instance: null, handle: null };
  }, []);

  /**
   * Compile callback handed to runPipeline. Lazy-mints the FossilPlayground
   * instance + opened file on first call; updateFile on subsequent calls so
   * Salsa's incremental memoisation kicks in across re-Runs of the same
   * mapping (the byte-identical-edit case = zero recompute).
   */
  const compile = useCallback(async (mappingText: string): Promise<string> => {
    const r = compileInstanceRef.current;
    if (!r.instance) {
      const instance = new FossilPlaygroundWasm();
      const handle = instance.openFile(documentUri, mappingText);
      compileInstanceRef.current = { instance, handle };
      return instance.compileFile(handle).sql;
    }
    if (r.handle !== null) {
      r.instance.updateFile(r.handle, mappingText);
    }
    return r.instance.compileFile(r.handle!).sql;
  }, []);

  // Boot the main-thread WASM module too (the Run path calls compileFile()
  // directly on the main thread; the LSP Worker boots its own WASM instance
  // separately).
  useEffect(() => {
    let cancelled = false;
    initFossilWasm({ wasmUrl })
      .then(() => {
        if (!cancelled) setWasmReady(true);
      })
      .catch((e) => {
        const err = e instanceof Error ? e : new Error(String(e));
        onError?.(err);
      });
    return () => {
      cancelled = true;
    };
  }, [wasmUrl, onError]);

  // Free the main-thread compile instance on unmount. Triggers Rust-side
  // Salsa store drop (per ADR-0026's intent — heap-heavy resources outside
  // React state are torn down on lifecycle boundaries, not garbage-collected
  // opportunistically).
  useEffect(() => {
    return () => {
      freeCompileInstance();
    };
  }, [freeCompileInstance]);

  // Compose CodeMirror extensions. Per ADR-0032 + 08-08:
  //   - fossil({ resolver }) provides syntactic highlighting + @-autocomplete
  //   - languageServerSupport(client, uri) provides diagnostics + semantic
  //     tokens overlay + LSP completion (the @codemirror/lsp-client extensions)
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
   * Run handler. Delegates the four-step pipeline (compile → resolve+rewrite →
   * DuckDB execute → vertex/edge readback) to `runPipeline`. The four-step
   * separation lives in `../run/runPipeline.ts` so:
   *
   *   - the pipeline can be reused by advanced consumers building custom
   *     layouts via the sub-components (the advanced composition path from
   *     08-09 SUMMARY — same surface, different chrome);
   *   - the orchestration is unit-testable without a real WASM or DuckDB
   *     boot (the `compile` and `getDuckDb` deps are injected here);
   *   - this component stays focused on React state / ARIA / lifecycle
   *     concerns.
   *
   * Per CONN-01: the runPipeline never stores resolved URLs in React state.
   * It substitutes them inline into the SQL string then hands the SQL to
   * DuckDB; the URLs disappear with the local `transformed.executableSql`
   * variable when runPipeline returns.
   */
  async function handleRun(): Promise<void> {
    setRunError(null);
    // Announce start to screen readers (polite live region). The user
    // can hear "Compiling and running mapping..." while focus stays on
    // the Run button — no focus jump required.
    const clearStartAnnouncement = announce('Compiling and running mapping…');
    try {
      if (!wasmReady) {
        throw new Error(
          'WASM is still loading — please wait a moment and try again.',
        );
      }
      const result = await runPipeline(
        { getDuckDb, compile },
        { resolver, mapping, maxResolvedBytes },
      );
      setVertices(result.vertices);
      setEdges(result.edges);
      onRun?.(result);
      announce(
        `Run complete: ${result.vertices.length} vertices, ${result.edges.length} edges.`,
      );
    } catch (e) {
      const err = e instanceof Error ? e : new Error(String(e));
      setRunError(err);
      onError?.(err);
      // Errors get an ASSERTIVE `role="alert"` block in the JSX below — the
      // live region announces the polite event-completion phrase so SR users
      // hear both: the polite "Run failed" + the assertive error details.
      announce(`Run failed: ${err.message}`);
    } finally {
      clearStartAnnouncement();
    }
  }

  function handleReset(): void {
    try {
      reset();
      // Free + null the main-thread compile instance so a fresh Run starts
      // from a clean Salsa store. Matches the spirit of ADR-0026 PLAY-12 —
      // the DuckDB Worker is recreated; the compile instance is light
      // enough to recreate too, and dropping it prevents Salsa state from
      // compounding across many Resets.
      freeCompileInstance();
      setVertices([]);
      setEdges([]);
      setRunError(null);
      announce('Playground reset.');
    } catch (e) {
      const err = e instanceof Error ? e : new Error(String(e));
      onError?.(err);
      announce(`Reset failed: ${err.message}`);
    }
  }

  const tabularFallback: ReactNode = (
    <ResultTable rows={vertices} caption="Vertices (tabular fallback)" />
  );

  return (
    <div
      className="fossil-playground"
      data-testid="fossil-playground"
      // role="application" tells AT this is a rich-interaction widget (the
      // CodeMirror editor + LSP-driven completions need direct key capture).
      // Per WAI-ARIA 1.2 application role: appropriate when the page contains
      // composite custom widgets where normal AT browse-mode would interfere
      // with intended interactions. The aria-label gives the role a name so
      // SR users hear "Fossil playground, application" on entry.
      role="application"
      aria-label="Fossil playground"
      // CSS custom properties applied at the root — every descendant
      // (including the CodeMirror editor host + ResultTable/Graph) reads from
      // here. Hosts can ALSO override the same `--fossil-*` variables at any
      // ancestor element via plain CSS; the cascade wins, so per-instance
      // overrides require no prop changes. Per THEME-01 + CONTEXT.md.
      style={cssVarsToStyle(cssVars)}
    >
      <header className="fossil-playground__toolbar" role="banner">
        <button
          type="button"
          onClick={() => {
            void handleRun();
          }}
          disabled={duck.loading}
          // Accessible name comes from ARIA_LABELS — the visible text inside
          // the button changes ("Run" → "Running…") but the accessible name
          // stays stable so SR users don't hear the label flip mid-interaction.
          aria-label={ARIA_LABELS.runButton}
          // aria-busy mirrors the disabled state so AT announces the busy
          // state in addition to the disabled state (some SR ignore disabled
          // buttons entirely; aria-busy is the canonical busy signal).
          aria-busy={duck.loading || undefined}
        >
          {duck.loading ? 'Running…' : 'Run'}
        </button>
        <button
          type="button"
          onClick={handleReset}
          aria-label={ARIA_LABELS.resetButton}
        >
          Reset playground
        </button>
      </header>
      <main className="fossil-playground__main">
        <section aria-label={ARIA_LABELS.editor}>
          {wasmReady ? (
            <FossilEditor
              value={mapping}
              onChange={setMapping}
              extensions={extensions}
            />
          ) : (
            // Gated on main-thread WASM init: CodeMirror's StreamParser
            // calls tokenize() eagerly at mount; rendering the editor
            // before initFossilWasm resolves crashes the parser. The
            // playground stays interactive (Run/Reset buttons render)
            // while the editor is loading.
            <div
              role="status"
              aria-live="polite"
              className="fossil-playground__editor-loading"
              style={{ padding: '1rem', color: 'var(--fossil-colors-muted)' }}
            >
              Loading editor…
            </div>
          )}
        </section>
        {(runError || duck.error) && (
          // role="alert" — ASSERTIVE announcement (interrupts whatever the
          // SR is currently saying). Reserved for genuine errors; routine
          // status changes use the polite announce() live region.
          <div
            role="alert"
            aria-live="assertive"
            className="fossil-playground__error"
          >
            Error: {(runError ?? duck.error)?.message}
          </div>
        )}
        <section
          aria-label={ARIA_LABELS.resultsRegion}
          className="fossil-playground__results"
        >
          <ResultGraph vertices={vertices} edges={edges} fallback={tabularFallback} />
          <ResultTable rows={edges} caption="Edges" />
        </section>
      </main>
    </div>
  );
}
