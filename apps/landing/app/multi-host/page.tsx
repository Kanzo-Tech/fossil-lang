'use client';

/**
 * `/multi-host` — Keasy-styled embedded-host fixture (REL-03 part B).
 *
 * Per 17-CONTEXT.md (locked decisions): "A NEW page in `apps/landing`
 * (e.g. `/multi-host` ...) that renders `<FossilEditor/>` +
 * `<FossilViewer/>` wrapped in a host shell that mimics Keasy's
 * layout/spacing/typography. ... It does NOT contain Keasy source code
 * — only Keasy's visual look."
 *
 * NOTE on naming: there is ALREADY a separate `apps/landing/tests/e2e/
 * multi-host-fixture/` directory containing a Phase 8 SC#2 Vite fixture
 * proving non-Next.js mounting. That fixture is UNRELATED to this route
 * — this is a Next.js route inside the landing app at the URL path
 * `/multi-host`, NOT a Vite fixture. Both can coexist; do not confuse
 * them when editing.
 *
 * Why two boundaries (page.tsx Client → MultiHostClient via dynamic):
 *
 *   `@fossil-lang/editor` + `@fossil-lang/viewer` + `@fossil-lang/wasm`
 *   reach for browser globals (`window`, `Worker`, `document`) — none of
 *   which exist during SSR. Per Next.js 15 (verified next@15.5.18),
 *   `dynamic({ ssr: false })` is NO LONGER allowed inside Server
 *   Components. The canonical fix is to make the route a Client
 *   Component itself (this file) and have it dynamic-import the
 *   WASM-bearing payload (`./multi-host-client.tsx`) with `ssr: false`.
 *
 *   This mirrors the structure of the main `/` route (apps/landing/app/
 *   page.tsx → ClientShell.tsx → PlaygroundHost.tsx) — except we don't
 *   need a `LandingHero` above the fixture, so we collapse the Server
 *   Component into the Client Component (one fewer file).
 *
 *   The `<KanzoThemeProvider/>` from `@kanzo/theme` is installed at this
 *   level so the shadcn token cascade (`var(--background)`,
 *   `var(--foreground)`, `var(--border)`, `var(--muted)`, `var(--primary)`,
 *   `var(--accent)`, etc.) wraps the entire fixture subtree — same brand
 *   wrap the playground host uses, applied here so the Keasy-styled
 *   shell's `keasy-shell.module.css` rules resolve their var() lookups.
 */

import dynamic from 'next/dynamic';
import { KanzoThemeProvider } from '@kanzo/theme';

const MultiHostClient = dynamic(() => import('./multi-host-client'), {
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
      Loading multi-host fixture…
    </div>
  ),
});

export default function MultiHostPage(): JSX.Element {
  return (
    <KanzoThemeProvider style={{ minHeight: '100vh' }}>
      <MultiHostClient />
    </KanzoThemeProvider>
  );
}
