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

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@fossil-lang/ui';

import { useLspWorker } from '../hooks/useLspWorker.js';
import { useDuckDb, getDuckDb } from '../hooks/useDuckDb.js';
import { useResetPlayground } from '../hooks/useResetPlayground.js';
import { useTheme } from '../hooks/useTheme.js';
import { usePermalink } from '../hooks/usePermalink.js';
import { cssVarsToStyle } from '../theme/tokens.js';
import { announce, ARIA_LABELS } from '../a11y/index.js';
import { runPipeline } from '../run/runPipeline.js';
import { CompiledSqlPanel } from '../compiled-sql/index.js';
// BibTeX cite modal (PLAY-08) — toolbar trigger + native <dialog> modal showing
// (BibtexModal import moved to Toolbar.tsx in Phase 14 plan 14-04)
// Phase 13 (ADR-0037) — host-side InferredDescriptor orchestration. Replaces
// the Phase 9 CSVW inference + editable preview. The hook scans the mapping
// for io.csv("...") / io.json("...") refs, runs DuckDB-WASM DESCRIBE, and
// calls registerInferredDescriptor on the WASM-side FossilPlayground BEFORE
// compile(). The CSVW directory (../csvw/) and its <CsvwPreview/> are gone
// in this commit; v0.1 .fossil files with explicit `schema = "..."` still
// compile (with a D-CSVW-DEPRECATED warning surfaced from fossil-hir per
// plan 13-02).
import { useInferredDescriptors } from '../hooks/useInferredDescriptors.js';
// Phase 14 plan 14-03 — IDE-style tabs layout. The playground composition
// shells out to dedicated panel sub-components for each tab pane so the
// component stays focused on orchestration (state + run pipeline + permalink).
import { MappingPanel } from './MappingPanel.js';
import { SourcePanel, type SourceSchema } from './SourcePanel.js';
import { ShapePanel } from './ShapePanel.js';
import { OutputPanel } from './OutputPanel.js';
import { Toolbar } from './Toolbar.js';

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
 * Discriminator for the left-side IDE panel tabs (Phase 14 plan 14-03 /
 * COMP-03). The Mapping tab carries the editor; Source shows the DuckDB
 * DESCRIBE preview of resolved sources; Shape carries a write-capable
 * editor over the ShEx target shape.
 */
type LeftTabKey = 'mapping' | 'source' | 'shape';

/**
 * Discriminator for the right-side IDE panel tabs. Output renders the
 * `<FossilViewer/>` (Graph / Turtle / Vertices / Edges sub-tabs from
 * Phase 12); Compiled SQL renders the always-mounted-but-tab-gated
 * `<CompiledSqlPanel/>`.
 */
type RightTabKey = 'output' | 'compiled-sql';

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
  // the `<CompiledSqlPanel/>` rendered inside the right-panel tab. The value
  // is recomputed via a 200 ms debounced effect over `mapping` whenever the
  // Compiled SQL tab is active.
  const [compiledSql, setCompiledSql] = useState<string>('');
  // Phase 14 plan 14-03: IDE-tab discriminators. Left panel default
  // 'mapping' is the .fossil source — the canonical landing surface. Right
  // panel default 'output' is the post-Run viewer (Graph/Turtle/Vertices/
  // Edges sub-tabs internally).
  const [activeLeftTab, setActiveLeftTab] = useState<LeftTabKey>('mapping');
  const [activeRightTab, setActiveRightTab] =
    useState<RightTabKey>('output');
  // Phase 14 plan 14-03: snapshot of the inferred source schemas — populated
  // by handleRun after `inferredDescriptors.introspectAndRegister(...)`
  // returns (the hook captures the schemas during the same DuckDB DESCRIBE
  // pass that registers them with the WASM instance). Read by SourcePanel.
  const [sourceSchemas, setSourceSchemas] = useState<SourceSchema[]>([]);
  const [sourceLoading, setSourceLoading] = useState<boolean>(false);
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
   * Ensure the main-thread `FossilPlaygroundWasm` instance + opened file
   * exist; mint on first call, `updateFile` on subsequent calls so Salsa's
   * incremental memoisation kicks in across re-Runs of the same mapping
   * (the byte-identical-edit case = zero recompute).
   *
   * Returns the live instance so the caller can drive pre-compile orchestration
   * (Phase 14 plan 14-01: `inferredDescriptors.introspectAndRegister(...)`
   * is awaited against this instance BEFORE the `compile` callback is invoked
   * via `runPipeline`).
   */
  const ensureCompileInstance = useCallback(
    (mappingText: string): FossilPlaygroundWasm => {
      const r = compileInstanceRef.current;
      if (!r.instance) {
        const instance = new FossilPlaygroundWasm();
        const handle = instance.openFile(documentUri, mappingText);
        compileInstanceRef.current = { instance, handle };
        return instance;
      }
      if (r.handle !== null) {
        r.instance.updateFile(r.handle, mappingText);
      }
      return r.instance;
    },
    [],
  );

  /**
   * Compile callback handed to runPipeline. Reuses `ensureCompileInstance`
   * so the lazy-mint logic + Salsa-incremental-memoisation contract live in
   * a single place.
   */
  const compile = useCallback(
    async (mappingText: string): Promise<string> => {
      const instance = ensureCompileInstance(mappingText);
      const handle = compileInstanceRef.current.handle!;
      return instance.compileFile(handle).sql;
    },
    [ensureCompileInstance],
  );

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
    // Phase 14 plan 14-03: gating switched from the standalone
    // `showCompiledSql` boolean to the right-panel tab discriminator. When
    // the Compiled SQL tab is not active, this effect is a no-op (no
    // wasted compile-per-keystroke + no shared-instance collision with
    // the Run path).
    if (activeRightTab !== 'compiled-sql') return;
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
  }, [mapping, wasmReady, compile, activeRightTab]);

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
      // Phase 14 plan 14-01 — wire the Phase 13 deferred follow-up. The
      // host-side InferredDescriptor flow (ADR-0037 / plan 13-04b) must run
      // BEFORE WASM `compile()` so Salsa's forward-propagation of source
      // types sees fresh `registerInferredDescriptor(...)` calls instead of
      // falling back to the deprecated CSVW codepath.
      //
      // Hoist the WASM instance mint here (rather than letting `compile`
      // lazy-mint on first call inside `runPipeline`) so the introspect step
      // can target the same instance the subsequent `compile` callback uses.
      // Per the `useInferredDescriptors` contract: per-source failures are
      // logged + skipped internally — the promise resolves successfully even
      // when individual sources fail. No try/catch wrap needed at the call
      // site; the existing outer catch handles `ensureCompileInstance` failure
      // (e.g. WASM not ready, mint throw).
      //
      // Phase 14 plan 14-03: the hook also returns the captured schemas so
      // the Source tab's preview can render without a second DESCRIBE pass.
      const instance = ensureCompileInstance(mapping);
      setSourceLoading(true);
      let schemas: SourceSchema[] = [];
      try {
        schemas = await inferredDescriptors.introspectAndRegister(
          mapping,
          instance,
        );
      } finally {
        setSourceLoading(false);
      }
      setSourceSchemas(schemas);
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
      // Phase 14 plan 14-03: clear the source-schema snapshot too — keeping
      // stale schemas across a Reset would mismatch the freshly-recreated
      // WASM instance's Salsa store.
      setSourceSchemas([]);
      announce('Playground reset.');
    } catch (e) {
      const err = e instanceof Error ? e : new Error(String(e));
      onError?.(err);
      announce(`Reset failed: ${err.message}`);
    }
  }

  // Phase 14 plan 14-03: The legacy `tabularFallback` (ResultTable wrapper)
  // and the Turtle vertex/edge adapters are GONE — the post-Run viewer is
  // now delegated entirely to `<FossilViewer/>` from `@fossil-lang/viewer`
  // (Phase 12), which owns its own Graph / Turtle / Vertices / Edges sub-
  // tabs plus TabularFallback. The result panel composition collapses to
  // a single `<OutputPanel/>` call.

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
      <Toolbar
        onRun={() => {
          void handleRun();
        }}
        onReset={handleReset}
        running={duck.loading}
        permalink={currentPermalink}
      />
      <main
        className="fossil-playground__main"
        style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}
      >
        <ResizablePanelGroup
          direction="horizontal"
          autoSaveId="fossil-playground-split"
          style={{ flex: 1, minHeight: 0 }}
        >
          <ResizablePanel
            defaultSize={50}
            minSize={20}
            data-testid="resizable-left"
          >
            <div
              className="fossil-playground__left"
              style={{ height: '100%', overflow: 'hidden' }}
            >
              <Tabs
                value={activeLeftTab}
                onValueChange={(v) => {
                  setActiveLeftTab(v as LeftTabKey);
                }}
              >
                <TabsList variant="line" aria-label="Input panels">
                  <TabsTrigger value="mapping" data-testid="ide-tab-mapping">
                    Mapping
                  </TabsTrigger>
                  <TabsTrigger value="source" data-testid="ide-tab-source">
                    Source
                  </TabsTrigger>
                  <TabsTrigger value="shape" data-testid="ide-tab-shape">
                    Shape
                  </TabsTrigger>
                </TabsList>
                <TabsContent value="mapping">
                  <MappingPanel
                    value={mapping}
                    onChange={setMapping}
                    extensions={extensions}
                    wasmReady={wasmReady}
                  />
                </TabsContent>
                <TabsContent value="source">
                  <SourcePanel
                    schemas={sourceSchemas}
                    loading={sourceLoading}
                  />
                </TabsContent>
                <TabsContent value="shape">
                  <ShapePanel
                    shex={shex}
                    onChange={setShex}
                    resolver={resolver}
                    wasmReady={wasmReady}
                  />
                </TabsContent>
              </Tabs>
            </div>
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel
            defaultSize={50}
            minSize={20}
            data-testid="resizable-right"
          >
            <section
              className="fossil-playground__right"
              aria-label={ARIA_LABELS.resultsRegion}
              style={{ height: '100%', overflow: 'hidden' }}
            >
              <Tabs
                value={activeRightTab}
                onValueChange={(v) => {
                  setActiveRightTab(v as RightTabKey);
                }}
              >
                <TabsList variant="line" aria-label="Output panels">
                  <TabsTrigger value="output" data-testid="ide-tab-output">
                    Output
                  </TabsTrigger>
                  <TabsTrigger
                    value="compiled-sql"
                    data-testid="ide-tab-compiled-sql"
                  >
                    Compiled SQL
                  </TabsTrigger>
                </TabsList>
                <TabsContent value="output">
                  <OutputPanel vertices={vertices} edges={edges} />
                </TabsContent>
                <TabsContent value="compiled-sql">
                  <div
                    id="fossil-compiled-sql-region"
                    aria-label="Compiled SQL"
                    className="fossil-playground__compiled-sql"
                  >
                    <CompiledSqlPanel
                      sql={compiledSql}
                      theme={resolvedTheme}
                    />
                  </div>
                </TabsContent>
              </Tabs>
            </section>
          </ResizablePanel>
        </ResizablePanelGroup>
        {(runError || duck.error) && (
          // role="alert" — ASSERTIVE announcement (interrupts whatever the
          // SR is currently saying). Reserved for genuine errors; routine
          // status changes use the polite announce() live region.
          <div
            role="alert"
            aria-live="assertive"
            className="fossil-playground__error"
            style={{ padding: '0.5rem' }}
          >
            Error: {(runError ?? duck.error)?.message}
          </div>
        )}
      </main>
    </div>
  );
}
