/// <reference lib="webworker" />
/**
 * Serwist Service Worker for the Fossil playground landing host.
 *
 * Discharges OFFLINE-01: the playground core (editor + LSP Worker + WASM
 * + DuckDB-WASM + bundled examples + CodeMirror Fossil mode) works
 * without network after first visit. Per CONTEXT.md non-functional
 * requirements + ADR-0028 reusable-component contract.
 *
 * Library choice (Serwist, NOT next-pwa):
 *   - `next-pwa` is unmaintained (last meaningful release 2023) per
 *     RESEARCH.md.
 *   - Serwist is the canonical Workbox successor with first-class
 *     Next.js 15 App Router support via `@serwist/next`.
 *
 * Precache strategy:
 *   - The default Serwist Next plugin precaches the App Router build
 *     manifest (HTML routes + chunks + static assets). The
 *     `self.__SW_MANIFEST` global is injected by `@serwist/next` at
 *     build time.
 *   - We layer a CacheFirst rule for .wasm files so the WASM bundle
 *     (largest single artefact ~2 MB) is served from cache forever
 *     (30-day expiration as a safety hatch).
 *
 * Fallback strategy:
 *   - Navigation requests that miss the cache fall back to `/~offline`
 *     — the Server Component page renders pure HTML with no JS needed.
 */
import { defaultCache } from '@serwist/next/worker';
import type { PrecacheEntry } from '@serwist/precaching';
import { CacheFirst, ExpirationPlugin, Serwist } from 'serwist';

declare const self: ServiceWorkerGlobalScope & {
  __SW_MANIFEST: (PrecacheEntry | string)[] | undefined;
};

const serwist = new Serwist({
  precacheEntries: self.__SW_MANIFEST,
  skipWaiting: true,
  clientsClaim: true,
  navigationPreload: true,
  runtimeCaching: [
    ...defaultCache,
    {
      // WASM bundle — CacheFirst because it's content-addressed at the
      // file level (fossil_wasm_bg.wasm changes name on every Rust
      // rebuild via wasm-bindgen's content-hash suffix in production).
      //
      // The `handler` must be an actual Strategy instance (per Serwist
      // types — `RouteHandler`), NOT a string. The plan-spec hinted at
      // Workbox-style `handler: 'CacheFirst'` which is the older
      // Workbox runtime-caching syntax; Serwist's modern v9 API takes
      // the class instance.
      matcher: ({ url }) => url.pathname.endsWith('.wasm'),
      handler: new CacheFirst({
        cacheName: 'fossil-wasm',
        plugins: [
          new ExpirationPlugin({
            // 30 days. The hash-suffixed filenames make this safe; if a
            // new build ships with a new hash, the old entry just expires
            // unused.
            maxAgeSeconds: 60 * 60 * 24 * 30,
            // Cap the cache size — keep at most 3 WASM versions (current
            // + previous + one buffer). Prevents unbounded growth on a
            // long-lived install.
            maxEntries: 3,
          }),
        ],
      }),
    },
  ],
  fallbacks: {
    entries: [
      {
        url: '/~offline',
        matcher: ({ request }) => request.destination === 'document',
      },
    ],
  },
});

serwist.addEventListeners();
