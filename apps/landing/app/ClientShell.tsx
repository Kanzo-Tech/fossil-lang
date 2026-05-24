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
 */

import dynamic from 'next/dynamic';

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
  return <PlaygroundHost />;
}
