/**
 * Multi-host SC#2 proof — mounts <FossilPlayground/> in a vanilla Vite +
 * React host with NO Next.js dependency.
 *
 * This is the STRUCTURAL proof of Phase 8 SC#2 from CONTEXT.md:
 *   "pnpm create vite my-host --template react-ts && pnpm add
 *    @fossil-lang/playground" + <FossilPlayground resolver={...}
 *    initialMapping="..." /> in any React 18+ host renders a working
 *    playground.
 *
 * If a future refactor adds a Next.js dependency to the @fossil-lang/*
 * graph (e.g., an accidental `import 'next/dynamic'` in the playground
 * package), THIS file fails to type-check or build — the gate is
 * structural.
 *
 * Vite-specific bits we exercise:
 *   - `?url` query suffix on the WASM artefact resolves to a stable URL
 *     in dev (the workspace symlink) + a hashed URL in build (Vite's
 *     asset pipeline)
 *   - The `?raw` query suffix is used INSIDE @fossil-lang/examples, not
 *     here — that's its concern, not the host's
 *   - createRoot from react-dom/client (React 18's concurrent root API)
 */
import { createRoot } from 'react-dom/client';
import { FossilPlayground } from '@fossil-lang/playground';
import { createDefaultResolver } from '@fossil-lang/resolvers';
import {
  buildResolverExamples,
  helloExample,
} from '@fossil-lang/examples';
// Vite-native `?url` import gives us the WASM file's bundler URL. Next.js
// uses a different mechanism (the next.config.mjs side-effect copy) —
// proving the component supports BOTH idioms via the wasmUrl prop.
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

const resolver = createDefaultResolver({
  examples: buildResolverExamples(),
});

const rootEl = document.getElementById('root');
if (!rootEl) {
  throw new Error('Multi-host fixture: #root not found in index.html');
}

createRoot(rootEl).render(
  <FossilPlayground
    resolver={resolver}
    initialMapping={helloExample.mapping}
    wasmUrl={wasmUrl}
  />,
);
