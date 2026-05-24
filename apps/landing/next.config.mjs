/**
 * Next.js 15 App Router config for @fossil-lang/landing.
 *
 * Two concerns:
 *
 *  1. Serwist Service Worker (OFFLINE-01). Serwist is the canonical Workbox
 *     successor for Next.js — `next-pwa` is unmaintained per RESEARCH.md.
 *     The SW source lives at `service-worker.ts`; Serwist emits to
 *     `public/sw.js` at build time.
 *
 *  2. WASM copy. The `.wasm` artefact must live at a STABLE URL so the SW
 *     can precache it via the manifest. We copy fossil_wasm_bg.wasm from
 *     `@fossil-lang/wasm/pkg/` to `public/wasm/` at config-load time (a
 *     side-effect that runs before webpack starts) so:
 *       - the URL is deterministic (`/wasm/fossil_wasm_bg.wasm`)
 *       - the SW manifest can name the URL explicitly
 *       - the client-side `initFossilWasm({ wasmUrl })` resolves cleanly
 *     Per CONTEXT.md interfaces — option (a) "Copy at build time", chosen
 *     over option (b) "Re-export the URL" which is more brittle.
 */
import withSerwistInit from '@serwist/next';
import { copyFileSync, mkdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

// Copy fossil_wasm_bg.wasm into public/wasm/ at config-load time. Fails
// soft with a warning so `next dev` still boots if the WASM hasn't been
// built yet (consumer just sees an empty editor on first load + a console
// warning telling them to run `pnpm --filter @fossil-lang/wasm build:wasm`).
try {
  mkdirSync(resolve(__dirname, 'public/wasm'), { recursive: true });
  copyFileSync(
    require.resolve('@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm'),
    resolve(__dirname, 'public/wasm/fossil_wasm_bg.wasm'),
  );
  // eslint-disable-next-line no-console
  console.log('[next.config] copied fossil_wasm_bg.wasm → public/wasm/');
} catch (e) {
  // eslint-disable-next-line no-console
  console.warn(
    '[next.config] WASM copy skipped — run `pnpm --filter @fossil-lang/wasm build:wasm` first:',
    e instanceof Error ? e.message : String(e),
  );
}

const withSerwist = withSerwistInit({
  swSrc: 'service-worker.ts',
  swDest: 'public/sw.js',
  // Skip SW generation during development for faster HMR. Production
  // builds always emit the SW.
  disable: process.env.NODE_ENV === 'development',
  // Bump precache size cap so the WASM bundle (~5 MB raw / ~1.5 MB
  // gzipped) is precached on first install rather than waiting for the
  // runtime CacheFirst rule to lazy-cache it. Caches the offline-first
  // guarantee: even the FIRST Run after the SW installs works offline.
  // Default 2 MB would skip the bundle silently with a build-time warning.
  maximumFileSizeToCacheInBytes: 8 * 1024 * 1024, // 8 MB
});

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Next.js 15 needs explicit allowlist for workspace symlinks that aren't
  // pre-compiled CJS. Without this, @fossil-lang/* packages would be
  // treated as external + tree-shaken away from the client bundle.
  transpilePackages: [
    '@fossil-lang/playground',
    '@fossil-lang/codemirror-fossil',
    '@fossil-lang/wasm',
    '@fossil-lang/resolvers',
    '@fossil-lang/examples',
    '@fossil-lang/types',
  ],
  webpack(config) {
    // `?raw` query imports — @fossil-lang/examples bundles its fixture
    // files (.fossil/.csv/.csvw.json/.shex) via Vite's `import x from
    // './foo.txt?raw'` convention (per the package's `src/types.d.ts`
    // ambient declaration). Webpack doesn't natively understand query
    // suffixes; this rule replicates the Vite semantics by serving the
    // file content as a UTF-8 string asset.
    //
    // resourceQuery is the canonical webpack 5 way to scope a rule to
    // query-string-suffixed requests; asset/source emits the file as
    // raw source text (default UTF-8). See
    // https://webpack.js.org/guides/asset-modules/#source-assets.
    config.module.rules.push({
      resourceQuery: /raw/,
      type: 'asset/source',
    });
    return config;
  },
};

export default withSerwist(nextConfig);
