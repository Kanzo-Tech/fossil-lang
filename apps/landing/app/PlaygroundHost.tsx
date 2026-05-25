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
 *
 * PLAY-05 Examples dropdown (plan 09-09 Task 4):
 *   - <ExampleSelector/> renders above the playground, enumerating the
 *     bundled `examples` array. On selection, the host swaps the
 *     `initialMapping` prop AND bumps a remount-key so the playground
 *     re-initialises with the new seed (otherwise `initialMapping` is
 *     captured only on first mount per the component's contract).
 *   - The URL fragment is cleared on example-load so the user gets a
 *     clean share URL: the playground's own onStateChange then emits a
 *     fresh permalink for the loaded example within ~200 ms (the same
 *     200 ms debounce as a manual edit).
 *   - Permalink precedence: a hash-bearing initial load wins over the
 *     example seed (the user explicitly shared that snapshot). Only
 *     once the user picks something from the dropdown do we override.
 */

import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import {
  buildResolverExamples,
  examples,
  helloExample,
} from '@fossil-lang/examples';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ExampleSelector } from './example-selector';

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

  // --- PLAY-05 Examples dropdown ----------------------------------------
  //
  // `selectedExampleId` is '' on first mount (= "Custom" placeholder; the
  // playground shows its built-in hello default). When the user picks an
  // example, we:
  //   1. capture the example's mapping (from the bundled `examples` array;
  //      no network — these are Vite `?raw` imports inlined at build time);
  //   2. bump `remountKey` so <FossilPlayground/> tears down + remounts
  //      with the new `initialMapping` (the prop is captured at mount only
  //      per the playground's documented contract);
  //   3. clear `window.location.hash` so the user gets a clean share URL.
  //      The playground will emit a fresh debounced permalink (~200 ms)
  //      for the loaded example's state, and the host will mirror it back
  //      via `updateHash` below.
  const [selectedExampleId, setSelectedExampleId] = useState<string>('');
  const [activeMapping, setActiveMapping] = useState<string>(
    helloExample.mapping,
  );
  const [remountKey, setRemountKey] = useState<number>(0);

  const handleExampleChange = useCallback((id: string): void => {
    setSelectedExampleId(id);
    if (!id) {
      // "Custom" — leave the editor alone; do not remount.
      return;
    }
    const ex = examples.find((e) => e.id === id);
    if (!ex) return;
    setActiveMapping(ex.mapping);
    setRemountKey((k) => k + 1);
    // Clear the URL fragment — the loaded example IS its own state, the
    // stale permalink no longer represents what's in the editor. The
    // playground's debounced onStateChange will write a fresh fragment
    // for the new content within ~200 ms.
    if (typeof window !== 'undefined') {
      window.history.replaceState(
        null,
        '',
        window.location.pathname + window.location.search,
      );
    }
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
    <>
      <ExampleSelector
        value={selectedExampleId}
        onChange={handleExampleChange}
      />
      <FossilPlayground
        key={remountKey}
        resolver={resolver}
        initialMapping={activeMapping}
        // WASM is copied into /wasm/ at Next.js build time (see next.config.mjs
        // side-effect). The Service Worker precaches it on first visit so
        // subsequent loads work offline (OFFLINE-01).
        wasmUrl="/wasm/fossil_wasm_bg.wasm"
        theme="light"
        // PLAY-04 wiring. On first mount only (remountKey === 0): if the
        // URL carried a `#...` permalink, hydrate from it; otherwise the
        // component defaults to `initialMapping`. After an example switch
        // (remountKey > 0) we deliberately do NOT pass `initialPermalink`
        // — the user picked an example, so any stale fragment is moot.
        initialPermalink={remountKey === 0 ? initialPermalink : undefined}
        onStateChange={updateHash}
      />
    </>
  );
}
