'use client';

/**
 * Client-Component wrapper that mounts <FossilPlayground/> against the
 * Tier-1 default resolver (bundled examples + public HTTPS, no
 * credentials) per ADR-0029.
 *
 * Why a separate Client Component (not inline in page.tsx):
 *   - Next.js 15 Server Components MUST NOT carry 'use client'; the
 *     `dynamic({ ssr: false })` boundary in page.tsx is what flips us
 *     into the browser-only path.
 *   - Keeps the SSR-side bundle slim — page.tsx pre-renders nothing
 *     except the loading shell, so the initial HTML response is tiny
 *     (improves SC#3 cold-load).
 *
 * PLAY-04 URL-fragment wiring:
 *   - On mount: read `window.location.hash.slice(1)` and pass it as
 *     `initialPermalink` to <FossilPlayground/>. The component decodes
 *     it via its internal usePermalink hook (which is the ONLY decode
 *     site — keeps the library window.location-agnostic per CONTEXT.md).
 *   - On state change: receive the debounced encoded permalink via
 *     `onStateChange`, write it to `window.location.hash` via
 *     `history.replaceState` (NOT pushState — don't pollute the back
 *     button; NOT direct `location.hash =` — that triggers a scroll
 *     jump because the browser tries to scroll to the matching id).
 *
 * The component already debounces the emit at 200ms; here we just
 * mirror the value into history. Any further debouncing would be
 * redundant.
 */

import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import { buildResolverExamples, helloExample } from '@fossil-lang/examples';
import { useCallback, useEffect, useMemo, useState } from 'react';

export default function PlaygroundHost(): JSX.Element {
  // Memoise the resolver — recreating it per render would invalidate the
  // playground's resolver-dependent state (autocomplete cache, etc.) and
  // cause unnecessary re-mount churn inside the CodeMirror extensions.
  const resolver = useMemo(
    () =>
      createDefaultResolver({
        examples: buildResolverExamples(),
        // Tier-1 surface only — no credentials, no host bridges. Future
        // Keasy migration replaces this with a Tier-2 resolver via
        // <FossilPlayground resolver={keasyResolver} />.
        publicBuckets: ['https://playground-public.kanzo.dev/'],
      }),
    [],
  );

  // --- URL-fragment <-> permalink (PLAY-04) ---------------------------
  //
  // initialPermalink starts undefined; we flip it on mount via the
  // useEffect below, AFTER hydration is past (window is unavailable
  // during the SSR pre-render and would crash if we read it at module
  // scope). Once flipped, the value never changes — usePermalink's
  // hydratedRef latches the decode to a single pass; even if React
  // re-renders the host with a new fragment from elsewhere, the
  // playground keeps the user's in-progress edits.
  const [initialPermalink, setInitialPermalink] = useState<string | undefined>(
    undefined,
  );
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const hash = window.location.hash.slice(1); // strip leading '#'
    if (hash) setInitialPermalink(hash);
  }, []);

  // Debounced (component-side) onStateChange handler — writes the
  // encoded permalink into the URL fragment. Using `history.replaceState`
  // instead of pushState keeps the back button clean (the user can still
  // navigate away without an N-deep history stack of every keystroke
  // burst). Using replaceState instead of `location.hash = ...` avoids
  // the browser's "scroll to the matching id" behaviour, which would
  // jump-scroll the page when no such id exists.
  //
  // Guard against the no-op case (hash already matches) so React Strict
  // Mode + Next.js Fast Refresh don't trip an extraneous history entry.
  const updateHash = useCallback((permalink: string): void => {
    if (typeof window === 'undefined') return;
    const newHash = '#' + permalink;
    if (window.location.hash === newHash) return;
    window.history.replaceState(
      null,
      '',
      window.location.pathname + window.location.search + newHash,
    );
  }, []);

  return (
    <FossilPlayground
      resolver={resolver}
      initialMapping={helloExample.mapping}
      // WASM is copied into /wasm/ at Next.js build time (see next.config.mjs
      // side-effect). The Service Worker precaches it on first visit so
      // subsequent loads work offline (OFFLINE-01).
      wasmUrl="/wasm/fossil_wasm_bg.wasm"
      theme="light"
      // PLAY-04 wiring. If the URL carried a `#...` permalink at load
      // time, hydrate from it; otherwise the component defaults to
      // helloExample. Every edit fires onStateChange (debounced 200ms
      // inside the component); we mirror into history.replaceState.
      initialPermalink={initialPermalink}
      onStateChange={updateHash}
    />
  );
}
