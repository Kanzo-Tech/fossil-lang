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
 */

import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import { buildResolverExamples, helloExample } from '@fossil-lang/examples';
import { useMemo } from 'react';

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

  return (
    <FossilPlayground
      resolver={resolver}
      initialMapping={helloExample.mapping}
      // WASM is copied into /wasm/ at Next.js build time (see next.config.mjs
      // side-effect). The Service Worker precaches it on first visit so
      // subsequent loads work offline (OFFLINE-01).
      wasmUrl="/wasm/fossil_wasm_bg.wasm"
      theme="light"
    />
  );
}
