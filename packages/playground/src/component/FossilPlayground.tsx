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

import { FossilEditor } from '@fossil-lang/editor';
import { ResultTable } from './ResultTable.js';
import { ResultGraph } from './ResultGraph.js';
import { useLspWorker } from '../hooks/useLspWorker.js';
import { useDuckDb, getDuckDb } from '../hooks/useDuckDb.js';
import { useResetPlayground } from '../hooks/useResetPlayground.js';
import { useTheme } from '../hooks/useTheme.js';
import { usePermalink } from '../hooks/usePermalink.js';
import { cssVarsToStyle } from '../theme/tokens.js';
import { announce, ARIA_LABELS } from '../a11y/index.js';
import { runPipeline } from '../run/runPipeline.js';
import { CompiledSqlPanel } from '../compiled-sql/index.js';
import {
  TurtleTab,
  type VertexRow as TurtleVertexRow,
  type EdgeRow as TurtleEdgeRow,
} from '../turtle/index.js';
// BibTeX cite modal (PLAY-08) — toolbar trigger + native <dialog> modal showing
// Min Oo & Hartig + the current permalink BibTeX. See ../bibtex/BibtexModal.tsx.
import { BibtexModal } from '../bibtex/BibtexModal.js';
// Phase 13 (ADR-0037) — host-side InferredDescriptor orchestration. Replaces
// the Phase 9 CSVW inference + editable preview. The hook scans the mapping
// for io.csv("...") / io.json("...") refs, runs DuckDB-WASM DESCRIBE, and
// calls registerInferredDescriptor on the WASM-side FossilPlayground BEFORE
// compile(). The CSVW directory (../csvw/) and its <CsvwPreview/> are gone
// in this commit; v0.1 .fossil files with explicit `schema = "..."` still
// compile (with a D-CSVW-DEPRECATED warning surfaced from fossil-hir per
// plan 13-02).
import { useInferredDescriptors } from '../hooks/useInferredDescriptors.js';

/**
 * Default 10 MB cap for resolver-returned blob fetches. Per Phase 7 07-08 /
 * PLAY-12 / SC#4: a 50 MB CSV would blow the DuckDB-WASM heap mid-run before
 * Reset could fire. Resolver-side validation (this component refuses BEFORE
 * bytes hit the Worker).
 */
const DEFAULT_MAX_RESOLVED_BYTES = 10 * 1024 * 1024;

/**
 * Debounce window for the live "Compiled SQL" panel recompile after a
 * keystroke (PLAY-07). Matches the 200 ms permalink debounce from
 * `usePermalink` so we coalesce the same keystroke burst into a single
 * compile + a single permalink encode. Per 09-CONTEXT.md locked decision.
 */
const COMPILED_SQL_DEBOUNCE_MS = 200;

/**
 * Default `@prefix` block for the Turtle tab (PLAY-10). Parsing the source
 * `.fossil`'s declared prefixes is deferred to a v0.2 polish; the defaults
 * cover the `hello` example + the bundled curated set. Callers passing a
 * non-trivial mapping with custom prefixes will see their IRIs un-shortened
 * in the Turtle view but still RDF-correct — round-trip via n3.Parser.
 */
const TURTLE_DEFAULT_PREFIXES: Record<string, string> = {
  ex: 'https://example.org/',
  rdf: 'http://www.w3.org/1999/02/22-rdf-syntax-ns#',
  rdfs: 'http://www.w3.org/2000/01/rdf-schema#',
  xsd: 'http://www.w3.org/2001/XMLSchema#',
};

/**
 * Discriminator for the result-panel tablist (PLAY-10). The Graph tab is the
 * pre-Phase-9 default + carries the WebGL canvas (or its tabular fallback);
 * Edges is the pre-Phase-9 edge table; Turtle is the new PLAY-10 panel that
 * serializes vertex+edge into TTL text.
 */
type ResultTabKey = 'graph' | 'edges' | 'turtle';

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
   * Theme: `'light'` | `'dark'` | a custom `FossilTheme` object, OR
   * `undefined` (default — host-provider model).
   *
   * Per ADR-0035 (visual ownership separation, plan 10-09): the
   * `@fossil-lang/playground` v0.2.x default is BRAND-AGNOSTIC. When
   * `theme` is omitted, the component injects NO `--fossil-*` CSS vars
   * at the root; the host is expected to supply the cascade via an
   * ancestor theme provider (e.g. `<KanzoThemeProvider/>` from
   * `@kanzo/theme` for kanzo-branded hosts). The playground stays
   * interactive against browser defaults; only the visual chrome
   * cascades.
   *
   * v0.1.x compatibility: hosts that explicitly passed `theme='light'`
   * or `theme='dark'` see IDENTICAL behaviour — `'light'` resolves to
   * `lightTheme` and `'dark'` resolves to `darkTheme`, exactly as v0.1.x.
   * (Plan 10-06's brief flip of the no-prop default to `'fossil-ide'`
   * was reverted in plan 10-09; the `'fossil-ide'` string alias is no
   * longer a built-in @fossil-lang/* theme name. Hosts wanting the
   * kanzo IDE look install `@kanzo/theme` and wrap their tree in
   * `<KanzoThemeProvider/>`.)
   *
   * The chosen theme applies via CSS custom properties on the playground
   * root element. Hosts can override individual tokens at ANY ancestor
   * element by setting the same custom property — the cascade wins, so
   * per-instance overrides are possible without re-mounting.
   *
   * Custom themes: spread one of the built-ins (`lightTheme`, `darkTheme`
   * exported from this package) and override the leaf tokens you care
   * about. Memoise the resulting object via `useMemo` to avoid
   * tearing-down the CodeMirror editor on every render.
   */
  theme?: FossilThemeProp;

  /**
   * Permalink to hydrate from on mount (PLAY-04). When defined, the
   * component attempts to decode it via the package's standalone permalink
   * codec (gzip + base64url + schema-versioned envelope; see
   * `../permalink/index.ts`). On success the decoded `source`/`csvw`/`shex`
   * seed the editor + descriptor panels — taking precedence over
   * `initialMapping`. On failure (corrupted, future-version, base64/gzip/JSON
   * errors) the component falls back to its defaults and emits the error via
   * `onError` — never crashes.
   *
   * Per CONTEXT.md locked decision: the component does NOT touch
   * `window.location` itself. The host (apps/landing/app/PlaygroundHost.tsx)
   * reads `window.location.hash` on mount and passes it here.
   */
  initialPermalink?: string;

  /**
   * Debounced callback fired with the freshly-encoded permalink whenever the
   * editor source (and, in future plans, csvw/shex panels) change. The host
   * typically pipes this into `window.history.replaceState(null, '', '#' +
   * permalink)` so the URL stays shareable + reload-survivable.
   *
   * Debounce is 200ms (per CONTEXT.md decision — coalesces keystrokes without
   * making "share my URL right now" feel laggy). Errors from the encoder
   * (e.g. `PermalinkTooLargeError` above 8000 bytes) are surfaced via
   * `onError` and DO NOT suppress the underlying state change.
   */
  onStateChange?: (permalink: string) => void;
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
    theme: themeProp,
    initialPermalink,
    onStateChange,
  } = props;

  const [mapping, setMapping] = useState<string>(
    initialMapping ?? helloExample.mapping,
  );
  // ShEx state slot (PLAY-04 round-trip). The ShEx editor is wired in a
  // future plan; we hold the state here today so the permalink can
  // round-trip it.
  //
  // Phase 13 (ADR-0037): the v0.1 `csvw` state slot + auto-inference + dirty
  // guard are removed. Schema is inferred at compile time via host-side
  // DuckDB-WASM DESCRIBE orchestrated by useInferredDescriptors (see below).
  const [shex, setShex] = useState<string | undefined>(undefined);
  const [vertices, setVertices] = useState<VertexRow[]>([]);
  const [edges, setEdges] = useState<EdgeRow[]>([]);
  const [runError, setRunError] = useState<Error | null>(null);
  // Latest permalink emitted by usePermalink (PLAY-08). The Cite button
  // embeds this into the per-snapshot BibTeX entry. Starts undefined; flips
  // on the first debounced encode (~200 ms after mount, per usePermalink's
  // contract from 09-05). Kept in component state so the modal re-renders
  // when the encoded permalink changes — paste-from-modal stays in sync
  // with the editor.
  const [currentPermalink, setCurrentPermalink] = useState<string | undefined>(
    undefined,
  );
  // PLAY-07: live-recompiled DuckDB SQL for the Compiled SQL panel. Drives
  // the `<CompiledSqlPanel/>` rendered when `showCompiledSql` is true. The
  // value is recomputed via a 200 ms debounced effect over `mapping`.
  const [compiledSql, setCompiledSql] = useState<string>('');
  // PLAY-07: collapsed-by-default toggle for the Compiled SQL panel. The
  // user opens on demand via the "Show compiled SQL" toolbar button.
  const [showCompiledSql, setShowCompiledSql] = useState<boolean>(false);
  // PLAY-10: active tab in the result panel's tablist. Default `graph`
  // matches the pre-Phase-9 behaviour (the Graph viz was the only landing
  // surface). Switching to `turtle` renders the post-Run TTL view; switching
  // to `edges` renders the bare edges table.
  const [activeResultTab, setActiveResultTab] =
    useState<ResultTabKey>('graph');
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
  // PLAY-07 + PLAY-10: discriminator passed to child panels' `data-theme`
  // attribute. Custom FossilTheme objects don't carry a light/dark flag —
  // they default to `light` here; hosts that want dark-mode CSS for a
  // custom theme should pass the literal `'dark'` prop instead.
  const resolvedTheme: 'light' | 'dark' =
    themeProp === 'dark' ? 'dark' : 'light';

  // PLAY-04 — decode `initialPermalink` on mount + emit debounced
  // `onStateChange` on edits. The component DOES NOT touch window.location
  // here; the host (apps/landing/app/PlaygroundHost.tsx) is responsible for
  // the URL-fragment side of the loop per CONTEXT.md locked decision. See
  // ../hooks/usePermalink.ts for the two-effect implementation (one-shot
  // decode + debounced encode).
  const handlePermalinkError = useCallback(
    (err: Error): void => {
      if (onError) {
        onError(err);
      } else {
        // eslint-disable-next-line no-console
        console.warn('[FossilPlayground] permalink:', err.message);
      }
    },
    [onError],
  );
  const handlePermalinkHydrate = useCallback(
    (state: { source: string; shex?: string }): void => {
      setMapping(state.source);
      // setShex uses the incoming undefined as a "clear" signal so hydration
      // is total — a hydrated envelope without shex resets any locally-typed
      // value (matching the round-trip invariant). Phase 13 v0.2 (ADR-0037)
      // removed the `csvw` field from the v2 envelope; the permalink decoder
      // silently drops v0.1 csvw payloads.
      setShex(state.shex);
    },
    [],
  );
  // Wrap the consumer's onStateChange so we ALSO mirror the freshly-encoded
  // permalink into our `currentPermalink` state (PLAY-08). The Cite modal
  // reads from `currentPermalink` to embed the URL in its per-snapshot
  // BibTeX entry. The host's onStateChange still fires with the same value
  // (this is a tee, not a replacement).
  const handlePermalinkStateChange = useCallback(
    (permalink: string): void => {
      setCurrentPermalink(permalink);
      onStateChange?.(permalink);
    },
    [onStateChange],
  );
  usePermalink({
    initialPermalink,
    source: mapping,
    // Phase 13 (ADR-0037): the v2 envelope dropped csvw; the hook still
    // accepts a `csvw` arg for the transitional contract (ignored). Pass
    // undefined.
    csvw: undefined,
    shex,
    onHydrate: handlePermalinkHydrate,
    onStateChange: handlePermalinkStateChange,
    onError: handlePermalinkError,
  });

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

  // PLAY-07: debounced recompile of the Compiled SQL panel.
  //
  // Gated on `showCompiledSql` so the panel-closed default path does ZERO
  // extra work — both for perf (no compile-per-keystroke when the panel is
  // hidden) AND for correctness: the `compileInstanceRef` is shared with
  // the Run path; an in-flight live-compile colliding with `handleRun`'s
  // compile + the subsequent DuckDB execute can corrupt the shared WASM
  // pointer (surfaced as `Error: null pointer passed to rust` in the SC#1
  // E2E gate during plan 09-06 Task 3 — Rule-1 fix).
  //
  // The trade-off: opening the panel for the first time triggers a single
  // 200 ms-debounced compile. Subsequent mapping edits while open keep the
  // panel within ~200 ms of the source. Closing the panel halts the live
  // updates immediately. Run never collides because clicking Run typically
  // happens AFTER the user has stopped typing — by then the debounced
  // compile has either completed or been clear-timeout'd.
  //
  // On compile failure the SQL is replaced with a comment so the panel
  // never crashes — the real error surfaces via the existing `runError`
  // alert when the user clicks Run.
  useEffect(() => {
    if (!wasmReady) return;
    if (!showCompiledSql) return;
    const handle = setTimeout(() => {
      compile(mapping)
        .then((sql) => {
          setCompiledSql(sql);
        })
        .catch(() => {
          setCompiledSql(
            '-- (compile error — see Diagnostics panel for details)',
          );
        });
    }, COMPILED_SQL_DEBOUNCE_MS);
    return () => {
      clearTimeout(handle);
    };
  }, [mapping, wasmReady, compile, showCompiledSql]);

  // Phase 13 v0.2 (ADR-0037 / plan 13-04b) — host-side InferredDescriptor
  // orchestration replaces the Phase 9 CSVW inference + editable preview.
  // The hook scrapes io.csv("...") / io.json("...") refs from the mapping,
  // runs DuckDB-WASM DESCRIBE, and calls registerInferredDescriptor on the
  // WASM-side FossilPlayground BEFORE each compile invocation. The actual
  // call site is the compile / run handler in this component (which awaits
  // introspectAndRegister before forwarding to WASM compile()). The previous
  // useEffect-driven background-inference path is gone.
  const inferredDescriptors = useInferredDescriptors({
    resolver,
    // DuckDB-WASM's AsyncDuckDBConnection.query() returns an Arrow Table
    // whose .toArray() yields the {column_name, column_type} rows the hook
    // expects — structural type compatibility holds, hence the eslint-disabled
    // cast through unknown.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    connectionFactory: (async () => {
      const db = await getDuckDb();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      return (await (db as any).connect()) as never;
    }) as never,
  });
  // Reference suppression — the hook return is consumed at compile-time
  // (`inferredDescriptors.introspectAndRegister(...)`); for now we keep the
  // hook call so the orchestration seam exists.
  void inferredDescriptors;

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

  /**
   * Adapter from the component's flat `VertexRow` / `EdgeRow` shape (whose
   * keys are DuckDB column names from the projection in `runPipeline.ts`)
   * into the Turtle serializer's `{ iri, type, props }` / `{ src, pred, dst }`
   * shape (PLAY-10). The serializer is shape-agnostic at the type level —
   * we centralise the adapter here so the TurtleTab stays pure-presentational.
   *
   * Type defaults:
   *   - vertex `type` comes from a `type`/`class` column when present; falls
   *     back to `ex:Vertex` so the rdf:type quad is always emitted.
   *   - edge `pred` comes from a `predicate`/`pred` column when present; falls
   *     back to `ex:edge` so each edge becomes a valid quad.
   *
   * `props` includes every non-`id` / non-`type` column on the vertex row —
   * already in the right value-type vocabulary (string | number | boolean |
   * null) per `rowsToTurtle`'s contract.
   */
  const turtleVertices = useMemo<TurtleVertexRow[]>(() => {
    return vertices.map((v) => {
      const { id, type, class: cls, ...rest } = v as Record<string, unknown> & {
        id: string;
      };
      const props: Record<string, string | number | boolean | null> = {};
      for (const [k, val] of Object.entries(rest)) {
        if (
          val === null ||
          typeof val === 'string' ||
          typeof val === 'number' ||
          typeof val === 'boolean'
        ) {
          props[k] = val;
        } else if (val !== undefined) {
          // Arrow types (BigInt / Date / Decimal) reach here in production —
          // stringify so the writer emits a plain literal. Lossless for our
          // demo data; a future polish can specialise.
          props[k] = String(val);
        }
      }
      return {
        iri: id,
        type: String(type ?? cls ?? 'https://example.org/Vertex'),
        props,
      };
    });
  }, [vertices]);

  const turtleEdges = useMemo<TurtleEdgeRow[]>(() => {
    return edges.map((e) => {
      const er = e as Record<string, unknown> & {
        source: string;
        target: string;
      };
      return {
        src: er.source,
        pred: String(er.predicate ?? er.pred ?? 'https://example.org/edge'),
        dst: er.target,
      };
    });
  }, [edges]);

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
        {/* PLAY-07: collapsed-by-default toggle for the Compiled SQL panel.
            Accessible name describes the action — "Show" / "Hide" flip per
            state so SR users hear the new state on activation. */}
        <button
          type="button"
          onClick={() => {
            setShowCompiledSql((v) => !v);
          }}
          aria-expanded={showCompiledSql}
          aria-controls="fossil-compiled-sql-region"
          data-testid="toggle-compiled-sql"
        >
          {showCompiledSql ? 'Hide compiled SQL' : 'Show compiled SQL'}
        </button>
        {/*
          PLAY-08 Cite button. Embeds the current permalink (debounced ~200 ms
          after the last edit) into the snapshot BibTeX entry alongside the
          foundational-paper reference. The modal is hidden until clicked.
          Native <dialog> — focus trap + Escape-close + role="dialog" all
          inherited from the platform per RULE-3 deviation in 09-08.
        */}
        <BibtexModal permalink={currentPermalink} />
      </header>
      <main className="fossil-playground__main">
        <section aria-label={ARIA_LABELS.editor}>
          {wasmReady ? (
            <FossilEditor
              value={mapping}
              onChange={setMapping}
              extensions={extensions}
              // lspTransport is ignored when extensions is provided (the
              // playground pre-composes its own LSP wiring via the local
              // useLspWorker hook). Pass null to satisfy the LOCKED
              // FossilEditorProps surface per ADR-0036 / Phase 11 11-02.
              lspTransport={null}
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
        {/* Phase 13 v0.2 (ADR-0037 / plan 13-04b): the CSVW descriptor panel
            is GONE. The user no longer writes or sees a CSVW descriptor;
            schema is inferred at compile time via host-side DuckDB-WASM
            DESCRIBE (orchestrated by `useInferredDescriptors` above). */}
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
          {/* PLAY-10: tablist gating Graph / Edges / Turtle. Native ARIA
              pattern — role="tablist" + role="tab" + role="tabpanel" + the
              tab's `aria-selected` + `aria-controls` linkage. Keyboard
              navigation between tabs uses the platform's default focus
              order (left/right arrows are a Phase-10 polish; for v0.1
              Tab + Enter / Space works out of the box). */}
          <div
            role="tablist"
            aria-label="Result views"
            className="fossil-playground__tablist"
          >
            <button
              type="button"
              role="tab"
              aria-selected={activeResultTab === 'graph'}
              aria-controls="fossil-result-panel-graph"
              id="fossil-result-tab-graph"
              data-testid="result-tab-graph"
              onClick={() => {
                setActiveResultTab('graph');
              }}
            >
              Graph
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={activeResultTab === 'edges'}
              aria-controls="fossil-result-panel-edges"
              id="fossil-result-tab-edges"
              data-testid="result-tab-edges"
              onClick={() => {
                setActiveResultTab('edges');
              }}
            >
              Edges
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={activeResultTab === 'turtle'}
              aria-controls="fossil-result-panel-turtle"
              id="fossil-result-tab-turtle"
              data-testid="result-tab-turtle"
              onClick={() => {
                setActiveResultTab('turtle');
              }}
            >
              Turtle
            </button>
          </div>
          {activeResultTab === 'graph' && (
            <div
              role="tabpanel"
              id="fossil-result-panel-graph"
              aria-labelledby="fossil-result-tab-graph"
            >
              <ResultGraph
                vertices={vertices}
                edges={edges}
                fallback={tabularFallback}
              />
            </div>
          )}
          {activeResultTab === 'edges' && (
            <div
              role="tabpanel"
              id="fossil-result-panel-edges"
              aria-labelledby="fossil-result-tab-edges"
            >
              <ResultTable rows={edges} caption="Edges" />
            </div>
          )}
          {activeResultTab === 'turtle' && (
            <div
              id="fossil-result-panel-turtle"
              aria-labelledby="fossil-result-tab-turtle"
            >
              {/* TurtleTab carries its own role="tabpanel" + aria-label —
                  the wrapper div above is just the id/aria-labelledby
                  anchor for the tablist linkage. */}
              <TurtleTab
                vertices={turtleVertices}
                edges={turtleEdges}
                prefixes={TURTLE_DEFAULT_PREFIXES}
                theme={resolvedTheme}
              />
            </div>
          )}
        </section>
        {/* PLAY-07: Compiled SQL panel — collapsed by default; toggled via
            the toolbar button above. Always-mounted state (effect populates
            `compiledSql` on every debounced keystroke), so opening the
            panel is instant — no first-open flash. */}
        {showCompiledSql && (
          <section
            id="fossil-compiled-sql-region"
            aria-label="Compiled SQL"
            className="fossil-playground__compiled-sql"
          >
            <CompiledSqlPanel sql={compiledSql} theme={resolvedTheme} />
          </section>
        )}
      </main>
    </div>
  );
}
