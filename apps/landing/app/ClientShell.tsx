'use client';

/**
 * Thin Client Component that dynamic-imports the WASM-bearing
 * <PlaygroundHost/> with `ssr: false`.
 *
 * Per Next.js 15 (verified next@15.5.18), `dynamic({ ssr: false })`
 * inside a Server Component throws a build error. The canonical fix is
 * to host the call inside a Client Component — this one — and have the
 * Server Component render the Client Shell directly.
 *
 * The Client Shell does almost nothing except the dynamic import. The
 * actual playground composition lives in `PlaygroundHost.tsx`.
 *
 * Why split into THREE files (page.tsx Server → ClientShell.tsx Client
 * → PlaygroundHost.tsx Client) rather than collapse ClientShell + Host:
 *
 *   - `dynamic({ ssr: false })` defers loading until the browser asks
 *     for it. Collapsing means the host loads synchronously on every
 *     route hit, blocking first-paint with the WASM + CodeMirror chunks.
 *   - The split keeps the SSR-side HTML response tiny (just the header
 *     + the loading shell) which protects SC#3 cold-load.
 *   - Service Worker can precache the chunks separately + invalidate
 *     them independently.
 *
 * Service Worker takeover orchestration (SW-FIRST-LOAD-01):
 *
 *   The shell is the canonical single source of truth for the page
 *   lifecycle — it's mounted once for the whole route and never unmounts
 *   while the user is on the playground. That makes it the right place
 *   for the `controllerchange` listener that pairs with Serwist's
 *   `skipWaiting: true` + `clientsClaim: true` in `service-worker.ts`.
 *
 *   Without this pairing, RESEARCH.md Pitfall 3 bites: `clients.claim()`
 *   immediately takes control of every open tab — including tabs that
 *   loaded under the previous SW — and serves mixed-version assets to
 *   the older tab. With the listener, the older tab reloads on takeover
 *   and converges on the new SW (see `sw-multitab.spec.ts`).
 *
 *   The listener also closes Phase 8 carry-forward SW-FIRST-LOAD-01:
 *   `offline.spec.ts:98` ("SW registered + controlling on first load")
 *   flaked at `--workers=2` because the SW activated silently and the
 *   first page-load never re-resolved its `.controller`. With the
 *   reload-on-controllerchange, the first load deterministically
 *   transitions to a controlled state.
 */

import dynamic from 'next/dynamic';
import { useEffect } from 'react';
import { KanzoThemeProvider } from '@kanzo/theme';

const PlaygroundHost = dynamic(() => import('./PlaygroundHost'), {
  ssr: false,
  loading: () => (
    <div
      role="status"
      aria-live="polite"
      style={{
        padding: '2rem',
        textAlign: 'center',
        color: '#64748b',
      }}
    >
      Loading playground…
    </div>
  ),
});

export function ClientShell(): JSX.Element {
  // SW-FIRST-LOAD-01 — pair with `skipWaiting + clientsClaim` in
  // service-worker.ts. RESEARCH.md Pitfall 3 multi-tab safety + Phase 8
  // deferred-items.md first-load gate.
  //
  // Placed BEFORE any other effects in the component tree (this is the
  // outermost client mount) so listener registration races with SW
  // activation are minimized. The hook is idempotent and the `reloaded`
  // guard handles React 18 Strict Mode double-invocation.
  //
  // We do NOT call `navigator.serviceWorker.register()` here — Serwist's
  // `@serwist/next` runtime handles registration automatically via the
  // injected `__SW_MANIFEST` and the next.config.mjs withSerwist wrap.
  useEffect(() => {
    if (
      typeof navigator === 'undefined' ||
      !('serviceWorker' in navigator)
    ) {
      return;
    }

    let reloaded = false;
    const onControllerChange = (): void => {
      // Guard against double-fire under React 18 Strict Mode dev re-mount
      // AND against the (rare) browser firing controllerchange more than
      // once during a single update cycle.
      if (reloaded) return;
      reloaded = true;
      window.location.reload();
    };

    navigator.serviceWorker.addEventListener(
      'controllerchange',
      onControllerChange,
    );
    return () => {
      navigator.serviceWorker.removeEventListener(
        'controllerchange',
        onControllerChange,
      );
    };
  }, []);

  // Wrap the dynamic-imported playground in <KanzoThemeProvider/> from
  // @kanzo/theme. Per ADR-0035 (visual ownership separation, plan 10-09):
  // brand visuals are owned by the @kanzo/* family; @fossil-lang/playground
  // is brand-agnostic. apps/landing is a KANZO-branded reference host, so
  // it installs the brand cascade here — every descendant (including
  // <FossilPlayground/> and its @fossil-lang/ui primitives) inherits the
  // --fossil-* CSS vars via the Provider's wrapping div's inline style.
  //
  // Keeping the brand wrap one layer up from PlaygroundHost (rather than
  // inside PlaygroundHost itself) makes the OSS playground host file
  // portable to a non-kanzo deployment — replacing this ClientShell with a
  // different brand wrapper is the host-side knob.
  return (
    <KanzoThemeProvider style={{ minHeight: '100%' }}>
      <PlaygroundHost />
    </KanzoThemeProvider>
  );
}
