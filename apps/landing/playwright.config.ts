/**
 * Playwright config for the Fossil landing E2E gate.
 *
 * Two web servers run concurrently:
 *
 *   1. landing — the Next.js 15 production build on :3000. Serves the
 *      App Router app + the SW. Specs landing-run.spec.ts +
 *      a11y.spec.ts + offline.spec.ts target this.
 *   2. multi-host-fixture — the standalone Vite + React app on :4173.
 *      Serves the SC#2 host-agnostic proof. Spec multi-host.spec.ts
 *      targets this.
 *
 * CI runs with 2 workers (Playwright's default for CI was 1; bumping to 2
 * roughly halves the suite duration since the Run path's 5 s budget is
 * the long pole). Worker count is set via env (or here as default) so we
 * don't fan out so wide the cold WASM compile thrashes.
 *
 * Retries on CI handle flaky DuckDB-WASM cold starts (the first Run after
 * a fresh page load can occasionally lose to a Worker boot race).
 */
import { defineConfig, devices } from '@playwright/test';

// Ports are configurable via env so local devs whose port 3000 is occupied
// (Docker, OrbStack, another Next.js app, OIDC reverse-proxy, etc.) can
// override without editing the file. Defaults stay at high-numbered ports
// that are unlikely to collide on any sane workstation.
const LANDING_PORT = Number(process.env.LANDING_PORT ?? 3100);
const FIXTURE_PORT = Number(process.env.FIXTURE_PORT ?? 4173);

export default defineConfig({
  testDir: './tests/e2e',
  // Per-file parallelism. Spec count is small (4) so this is a wash; left
  // on for forwards-compatibility with Phase 9's growing suite.
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  use: {
    baseURL: process.env.BASE_URL || `http://localhost:${LANDING_PORT}`,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: [
    {
      // Next.js production server. `next start` requires `next build` to
      // have run; in CI the workflow calls `pnpm -r build` first.
      //
      // The PORT env var is the most reliable way to pass a port to a
      // pnpm-invoked Next.js script — passing `-- --port N` collides with
      // pnpm's own `--` separator parsing. Next.js's CLI honours PORT
      // (https://nextjs.org/docs/app/api-reference/cli/next#next-start-options).
      command: 'pnpm --filter @fossil-lang/landing start',
      env: { PORT: String(LANDING_PORT) },
      port: LANDING_PORT,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
      stdout: 'pipe',
      stderr: 'pipe',
    },
    {
      // Multi-host fixture (Vite preview). `vite build` must have run
      // first — CI workflow calls `pnpm -r build` which builds every
      // workspace member including the fixture.
      //
      // Vite's preview CLI takes `--port` directly; we pass it through a
      // wrapper invocation rather than through pnpm's `--` separator.
      command: `cd tests/e2e/multi-host-fixture && pnpm exec vite preview --port ${FIXTURE_PORT} --strictPort`,
      port: FIXTURE_PORT,
      reuseExistingServer: !process.env.CI,
      timeout: 60_000,
      stdout: 'pipe',
      stderr: 'pipe',
    },
  ],
  // 30 s per-test default. The Run path has its own 5 s assertion budget;
  // 30 s leaves headroom for cold-start + retries.
  timeout: 30_000,
  expect: {
    timeout: 5_000,
    // Phase 15 plan 15-05 visual baselines: 2% pixel-diff tolerance per
    // 15-CONTEXT.md ("≤2% pixel diff" target). This applies to every
    // `toHaveScreenshot()` assertion across the suite; visual-baselines.spec.ts
    // is the only current consumer but the Phase 17 cross-host suite (REL-03)
    // will inherit the same threshold.
    toHaveScreenshot: {
      maxDiffPixelRatio: 0.02,
    },
  },
  // Phase 15 plan 15-05 visual baselines: golden screenshots live under
  // `apps/landing/tests/visual/__snapshots__/` (NOT the default
  // `<test-file>.spec.ts-snapshots/` next to the spec). This keeps all
  // visual artefacts in one tree so Phase 16's Keasy migration PR can
  // gate on the directory wholesale and `git status apps/landing/tests/visual/`
  // surfaces drift independently of the spec files themselves.
  //
  // Template substitutions:
  //   {testDir}       → /Users/.../apps/landing/tests/e2e
  //   {testFilePath}  → visual-baselines.spec.ts (file basename, no extension stripped)
  //   {arg}           → the name passed to `toHaveScreenshot('name.png')`
  //   {ext}           → empty (the `{arg}` already carries `.png`); included for safety
  snapshotPathTemplate:
    '{testDir}/../visual/__snapshots__/{testFilePath}/{arg}{ext}',
});
