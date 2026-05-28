'use client';

/**
 * MultiHostClient — the WASM-bearing client component that mounts
 * `<FossilEditor/>` + `<FossilViewer/>` inside the `<KeasyShell/>` chrome.
 *
 * Why this lives in a separate file (and is dynamic-imported with
 * `ssr: false` by `page.tsx`):
 *   - `@fossil-lang/editor` + `@fossil-lang/viewer` + `@fossil-lang/wasm`
 *     all reach for browser globals (`window`, `Worker`, `document`). They
 *     CANNOT run during SSR.
 *   - Next.js 15 forbids `dynamic({ ssr: false })` inside Server
 *     Components; the canonical fix is hosting the dynamic-import call
 *     inside a Client Component. `page.tsx` is that Client Component;
 *     this file is the actual WASM-bearing payload that `page.tsx`
 *     dynamic-imports.
 *
 * Wire shape (mirrors apps/landing/app/PlaygroundHost.tsx + the
 * playground's `FossilPlayground.tsx` internal Run loop):
 *   1. URL-param theme: `?theme=dark` → 'dark', everything else → 'light'.
 *   2. Resolver: Tier-1 default — bundled examples + public HTTPS, no
 *      credentials (per ADR-0029).
 *   3. LSP: `useLspWorker` from `@fossil-lang/playground` returns an
 *      LSPClient backed by the long-lived LSP Worker (ADR-0026). Compose
 *      `[fossil({ resolver }), languageServerSupport(client, uri,
 *      'fossil')]` and hand the pre-composed extensions to
 *      `<FossilEditor/>` (the playground-path composition; preserves the
 *      same wiring the main `/` route uses).
 *   4. Run: clicking "Run mapping" mints a `FossilPlaygroundWasm` instance
 *      on first call, then drives `runPipeline({ getDuckDb, compile },
 *      { resolver, mapping, maxResolvedBytes })`. Result feeds
 *      `<FossilViewer/>`'s `vertices` + `edges` props.
 *   5. Theme: the multi-host shell sits inside `<KanzoThemeProvider/>`
 *      (installed by page.tsx), so the shadcn token vocabulary
 *      (`var(--background)`, `var(--foreground)`, `var(--border)`, etc.)
 *      cascades into the shell + every descendant.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { fossil } from '@fossil-lang/codemirror-fossil';
import {
  initFossilWasm,
  FossilPlayground as FossilPlaygroundWasm,
} from '@fossil-lang/wasm';
import {
  FossilEditor,
  NullTransport,
} from '@fossil-lang/editor';
import { FossilViewer } from '@fossil-lang/viewer';
import {
  runPipeline,
  getDuckDb,
  useLspWorker,
  type VertexRow,
  type EdgeRow,
} from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import {
  buildResolverExamples,
  helloExample,
} from '@fossil-lang/examples';
import { languageServerSupport } from '@codemirror/lsp-client';
import type { Extension } from '@codemirror/state';

import { KeasyShell } from './keasy-shell';

const WASM_URL = '/wasm/fossil_wasm_bg.wasm';
const MAX_RESOLVED_BYTES = 10 * 1024 * 1024;

/**
 * Read the `?theme=` URL param at mount time. Mirrors PlaygroundHost.tsx
 * convention so the Phase 15 visual-baselines spec can drive both routes
 * with the same `?theme=dark` URL parameter.
 *
 * The `useSearchParams` hook from `next/navigation` would also work, but
 * a one-shot read at mount time matches PlaygroundHost's behaviour: the
 * theme is captured on first paint and survives until reload — a
 * route-level URL convention, not a runtime toggle.
 */
function readThemeFromUrl(): 'light' | 'dark' {
  if (typeof window === 'undefined') return 'light';
  const params = new URLSearchParams(window.location.search);
  return params.get('theme') === 'dark' ? 'dark' : 'light';
}

export default function MultiHostClient(): JSX.Element {
  const [theme] = useState<'light' | 'dark'>(() => readThemeFromUrl());
  const [mapping, setMapping] = useState<string>(helloExample.mapping);
  const [vertices, setVertices] = useState<VertexRow[]>([]);
  const [edges, setEdges] = useState<EdgeRow[]>([]);
  const [running, setRunning] = useState<boolean>(false);
  const [runError, setRunError] = useState<string | null>(null);
  const [wasmReady, setWasmReady] = useState<boolean>(false);

  // Tier-1 default resolver — bundled examples + public HTTPS, no creds.
  // Memoised so re-renders don't churn @-autocomplete caches inside the
  // CodeMirror extension. Mirrors PlaygroundHost.tsx exactly.
  const resolver = useMemo(
    () =>
      createDefaultResolver({
        examples: buildResolverExamples(),
        publicBuckets: ['https://playground-public.kanzo.dev/'],
      }),
    [],
  );

  // LSP Worker — module-singleton, long-lived per ADR-0026. Returns null on
  // first render, then flips to a concrete LSPClient on the next render once
  // the boot useEffect inside the hook lands. The hook is safe to call on
  // every route — internally it short-circuits via a module-level cache.
  const lspClient = useLspWorker({ wasmUrl: WASM_URL });

  // Boot the main-thread WASM module. Mirrors FossilPlayground.tsx's
  // mount-effect: we own a separate main-thread instance for the Run path
  // (the LSP Worker has its own instance inside the Worker).
  useEffect(() => {
    let cancelled = false;
    initFossilWasm({ wasmUrl: WASM_URL })
      .then(() => {
        if (!cancelled) setWasmReady(true);
      })
      .catch((e) => {
        // Don't crash the page; surface the error in the banner.
        if (!cancelled) {
          setRunError(
            `WASM init failed: ${e instanceof Error ? e.message : String(e)}`,
          );
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Compose CodeMirror extensions exactly the way FossilPlayground.tsx
  // does: fossil({ resolver }) for highlighting + @-autocomplete; LSP
  // support extension when the client is ready. Pre-composed so we can
  // hand the array via `extensions={...}` to <FossilEditor/> (playground
  // path, ADR-0036). We still pass `lspTransport={NullTransport}` to
  // satisfy the typed prop surface; the prop is ignored when `extensions`
  // is provided (per the EDIT-02 composition rule).
  const extensions = useMemo<Extension[]>(() => {
    const exts: Extension[] = fossil({ resolver });
    if (lspClient) {
      exts.push(
        languageServerSupport(
          lspClient,
          'file:///multi-host/main.fossil',
          'fossil',
        ),
      );
    }
    return exts;
  }, [resolver, lspClient]);

  // Main-thread compile instance — lazy-minted on the first Run, reused on
  // subsequent Runs (Salsa-stable). Freed on unmount so the WASM linear
  // memory doesn't leak if the user navigates away and back. Mirrors the
  // ADR-0026 component-scope lifecycle that FossilPlayground.tsx uses.
  const compileInstanceRef = useRef<FossilPlaygroundWasm | null>(null);
  useEffect(() => {
    return () => {
      const inst = compileInstanceRef.current;
      if (inst) {
        try {
          inst.free();
        } catch {
          // free() on an already-freed instance throws — swallow.
        }
        compileInstanceRef.current = null;
      }
    };
  }, []);

  const compile = useCallback(
    async (source: string): Promise<string> => {
      if (!compileInstanceRef.current) {
        compileInstanceRef.current = new FossilPlaygroundWasm();
      }
      return compileInstanceRef.current.compile(source).sql;
    },
    [],
  );

  const onRun = useCallback(async (): Promise<void> => {
    if (!wasmReady || running) return;
    setRunning(true);
    setRunError(null);
    try {
      const result = await runPipeline(
        { getDuckDb, compile },
        { resolver, mapping, maxResolvedBytes: MAX_RESOLVED_BYTES },
      );
      setVertices(result.vertices);
      setEdges(result.edges);
    } catch (e) {
      setRunError(e instanceof Error ? e.message : String(e));
    } finally {
      setRunning(false);
    }
  }, [wasmReady, running, compile, resolver, mapping]);

  return (
    <KeasyShell
      theme={theme}
      onRun={() => {
        void onRun();
      }}
      running={running || !wasmReady}
      errorMessage={runError}
      editorPane={
        wasmReady ? (
          <FossilEditor
            value={mapping}
            onChange={setMapping}
            extensions={extensions}
            // EDIT-02: when `extensions` is provided the lspTransport
            // prop is ignored (composition is owner-controlled). Pass
            // NullTransport to satisfy the typed contract — the
            // languageServerSupport extension above carries the actual
            // LSP wiring.
            lspTransport={new NullTransport()}
            resolver={resolver}
            className="fossil-editor"
          />
        ) : (
          <div
            style={{
              padding: '1rem',
              color: 'hsl(var(--muted-foreground, 215 16% 47%))',
            }}
          >
            Loading editor…
          </div>
        )
      }
      viewerPane={
        <FossilViewer
          // Adapt the playground's loose VertexRow/EdgeRow shapes (only
          // `id` / `source`+`target` guaranteed) into FossilViewer's
          // stricter shape (`id` + `type` + `label`). Mirrors the
          // adapter logic in packages/playground/src/component/
          // OutputPanel.tsx so the fixture's viewer renders the same way
          // the main `/` route does after a Run.
          vertices={vertices.map((v) => ({
            ...v,
            id: v.id,
            type: String((v as Record<string, unknown>).type ?? 'Unknown'),
            label: String((v as Record<string, unknown>).label ?? v.id),
          }))}
          edges={edges.map((e) => ({
            ...e,
            source: e.source,
            target: e.target,
            predicate:
              typeof (e as Record<string, unknown>).predicate === 'string'
                ? ((e as Record<string, unknown>).predicate as string)
                : undefined,
          }))}
        />
      }
    />
  );
}
