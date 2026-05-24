# @fossil-lang/landing

Reference Next.js 15 host for `@fossil-lang/playground`. Deploys to
`playground.kanzo.dev` (PKG-02). NOT published to npm.

## Architecture

- **Next.js 15 App Router**. Server Component (`app/page.tsx`) dynamic-imports
  the Client wrapper (`app/PlaygroundHost.tsx`) with `ssr: false` — per
  RESEARCH.md Pitfall 3, ANY WASM-using component needs this boundary or
  `next build` fails with `ReferenceError: window is not defined`.
- **Service Worker via Serwist** precaches WASM + JS bundles + the bundled
  hello example (OFFLINE-01). `next-pwa` is unmaintained — Serwist is the
  canonical Workbox successor with first-class App Router support.
- **WASM copied at build time** from `@fossil-lang/wasm/pkg/` into
  `public/wasm/` via a `next.config.mjs` side-effect that runs before
  webpack. The URL `/wasm/fossil_wasm_bg.wasm` is stable and the SW
  manifest can name it explicitly.

## Local dev

```bash
# 1. Build the WASM artefact (needed by next.config.mjs copy step)
pnpm --filter @fossil-lang/wasm build:wasm

# 2. Run the landing dev server
pnpm --filter @fossil-lang/landing dev
```

Visit http://localhost:3000.

> Note: SW is disabled in development for faster HMR (`disable: NODE_ENV ===
> 'development'` in next.config.mjs). To test the SW, run a production build
> (below).

## Production build

```bash
pnpm -r build                                          # all packages
pnpm --filter @fossil-lang/landing build               # next build
pnpm --filter @fossil-lang/landing start               # serve on :3000
```

The `next build` step:

1. Copies `fossil_wasm_bg.wasm` into `public/wasm/` (side-effect of loading
   `next.config.mjs`).
2. Runs the Serwist plugin which emits `public/sw.js` from
   `service-worker.ts`.
3. Builds the App Router app — RSC + client bundles.

## Deploy

Vercel (the canonical deploy target — `playground.kanzo.dev` lives there):

```bash
npx vercel --prod --cwd apps/landing
```

Custom domain config is out of scope for v0.1 (initial deploy uses the
auto-generated `*.vercel.app` URL).

## E2E tests (Playwright)

```bash
# One-time
pnpm --filter @fossil-lang/landing exec playwright install --with-deps chromium

# Run the suite
pnpm --filter @fossil-lang/landing test:e2e
```

Specs under `tests/e2e/`:

| Spec | Gate |
|------|------|
| `landing-run.spec.ts` | **SC#1** — Run produces vertex+edge tables within 5s |
| `multi-host.spec.ts` | **SC#2** — `<FossilPlayground/>` mounts in a non-Next.js Vite host (proves host-agnostic) |
| `a11y.spec.ts` | **A11Y-01** — WCAG 2.1 AA via `@axe-core/playwright`, excludes `#graph-canvas` + `.cm-content` |
| `offline.spec.ts` | **OFFLINE-01** — Service Worker keeps editor + Run working after `setOffline(true)` |

## Multi-host fixture

`tests/e2e/multi-host-fixture/` is a minimal Vite + React app (NOT
Next.js) that mounts `<FossilPlayground/>` to prove the component is
host-agnostic per Phase 8 SC#2. It's declared in the root
`pnpm-workspace.yaml` so the workspace dep `@fossil-lang/playground:
workspace:*` resolves; Playwright's `webServer` block boots its Vite
preview on :4173 alongside the Next.js host on :3000.
