/**
 * Phase 15 plan 15-05 — Playground visual-baselines spec.
 *
 * Captures Playwright golden screenshots of the post-Phase-14 playground
 * layout. These baselines are the visual safety net for Phase 16 (Keasy
 * migration) — the migration PR re-runs this spec and must produce ≤ 2%
 * pixel diff per 15-CONTEXT.md.
 *
 * # Dependency chain
 *
 * - 15-01 (BUG-01) MUST land BEFORE these baselines are captured. A
 *   screenshot of a buggy second-Run would mis-encode the canary; baselines
 *   reflect the FIXED state (commits 24e8178 + b2618f7 + 8d4be61). Plan
 *   15-05 declares `depends_on: [15-01]` in its frontmatter.
 * - 15-04 lazy-loaded `@fossil-lang/viewer` (commit c29679c). The Output
 *   tab now waits for the React.lazy chunk to fetch + the Cosmos.gl
 *   WebGL canvas to mount. Specs that screenshot the Output tab give the
 *   viewer extra settle time (see waitForViewerReady() below).
 *
 * # Theme switching
 *
 * Driven by the `?theme=dark` URL search param wired into PlaygroundHost.tsx
 * by Task 0 of this plan (commit d90b3df). The helper `gotoLandingWithTheme`
 * navigates to `/?theme=dark` for dark and `/` for light; the playground's
 * own `theme` prop wins over the <KanzoThemeProvider/> cascade installed by
 * ClientShell.tsx (per useTheme.ts L96-108).
 *
 * DO NOT use `page.emulateMedia({ colorScheme: 'dark' })` — the playground
 * reads its `theme` prop value, NOT `prefers-color-scheme`, so emulateMedia
 * would be a no-op against the playground (it would only flip browser-default
 * UI chrome like scrollbars, which is exactly the noise we want to AVOID).
 *
 * # Snapshot location
 *
 * PNG baselines live under `apps/landing/tests/visual/__snapshots__/` per
 * the `snapshotPathTemplate` configured in playwright.config.ts. The full
 * path for a single baseline is e.g.
 * `apps/landing/tests/visual/__snapshots__/visual-baselines.spec.ts/light-mapping-default.png`.
 *
 * # Threshold
 *
 * 2% pixel-diff tolerance (`maxDiffPixelRatio: 0.02`) configured at the
 * `expect.toHaveScreenshot` level in playwright.config.ts. Below this:
 * green. At/above: red, with a `<name>-diff.png` artefact generated for
 * inspection.
 *
 * # Regenerating baselines
 *
 * After a deliberate visual change (e.g., theme tweak, layout adjustment):
 *
 *     pnpm --filter @fossil-lang/landing exec playwright test \
 *       tests/e2e/visual-baselines.spec.ts --update-snapshots
 *
 * Then visually inspect the generated PNGs (no half-rendered editor, no
 * missing tab content, theme correctly applied) and commit them under
 * `apps/landing/tests/visual/__snapshots__/`.
 *
 * # Determinism gotchas (applied in beforeEach below)
 *
 * - Pin viewport to 1440 × 900 (a common laptop size, stable across local + CI).
 * - Disable CSS animations and transitions (avoids capturing mid-frame state).
 * - Wait for fonts to finish loading (avoids FOIT/FOUT noise).
 * - Wait for the WASM editor mount gate ("Loading editor…" absent).
 * - 200 ms post-tab-click settle window (Phase 10 motion tokens).
 *
 * # Phase 16 consumption
 *
 * Phase 16 (Keasy migration) re-runs this spec as a PR gate:
 *
 *     pnpm --filter @fossil-lang/landing exec playwright test \
 *       tests/e2e/visual-baselines.spec.ts
 *
 * Any cell that exceeds the 2% threshold blocks the PR. Phase 17 (REL-03)
 * extends this to a cross-host suite (Keasy host + landing host both
 * captured + cross-checked).
 */
import { expect, test, type Page } from '@playwright/test';

/**
 * Navigate to the landing playground with the requested theme.
 *
 * Relies on the Task-0 URL-param wiring in PlaygroundHost.tsx — the host
 * reads `?theme=dark` and forwards `'dark'` to `<FossilPlayground/>`'s
 * `theme` prop. The 'light' branch uses the bare `/` path so the
 * production happy-path baseline is captured (zero query string).
 */
async function gotoLandingWithTheme(
  page: Page,
  theme: 'light' | 'dark',
): Promise<void> {
  const path = theme === 'dark' ? '/?theme=dark' : '/';
  await page.goto(path);
}

/**
 * Wait for the playground to reach a mountable steady state.
 *
 * Mirrors the readiness gates from landing-run.spec.ts:
 *   1. The `data-testid="fossil-playground"` root is visible (dynamic
 *      import resolved past the Client Shell's loading fallback).
 *   2. The "Loading editor…" overlay is gone (the WASM init flipped
 *      `wasmReady` to true; CodeMirror's StreamParser can call tokenize
 *      without racing the WASM boot).
 *   3. Document fonts are loaded (avoids capturing FOIT/FOUT noise).
 */
async function waitForReady(page: Page): Promise<void> {
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });
  // Wait for fonts — `document.fonts.ready` is a Promise that resolves
  // once every font face declared in CSS is either loaded or failed.
  await page.evaluate(() => document.fonts.ready);
}

/**
 * Wait for the lazy-loaded viewer chunk (Cosmos.gl WebGL canvas) to
 * mount. Used by tests that screenshot the Output tab after a Run.
 *
 * Per Phase 15 plan 15-04, `@fossil-lang/viewer` is now React.lazy'd —
 * the chunk fetch + Cosmos.gl init add ~200-500 ms latency the first
 * time the Output tab is mounted. The Suspense fallback shows
 * "Loading viewer…" until the chunk resolves; we wait for it to vanish.
 */
async function waitForViewerReady(page: Page): Promise<void> {
  await expect(page.getByText('Loading viewer…')).toHaveCount(0, {
    timeout: 10_000,
  });
}

/**
 * Disable all CSS animations and transitions to capture stable frames.
 * Re-applied per test (page reload wipes the style tag).
 */
async function disableAnimations(page: Page): Promise<void> {
  await page.addStyleTag({
    content: `
      *, *::before, *::after {
        animation-duration: 0ms !important;
        animation-delay: 0ms !important;
        transition-duration: 0ms !important;
        transition-delay: 0ms !important;
      }
    `,
  });
}

/**
 * Pin the viewport to a stable size for all baselines. 1440 × 900 is a
 * common MacBook-class laptop resolution and is what was used to design
 * the post-Phase-14 IDE-tabs layout.
 */
const VIEWPORT = { width: 1440, height: 900 } as const;

test.describe('Visual baselines — playground v2 (post-Phase-14)', () => {
  test.beforeEach(async ({ page }) => {
    await page.setViewportSize(VIEWPORT);
  });

  // Stub test scaffold — Task 2 fills in the full matrix.
  test('SC#4: baseline — light theme, default state, Mapping tab', async ({
    page,
  }) => {
    await gotoLandingWithTheme(page, 'light');
    await waitForReady(page);
    await disableAnimations(page);
    await page.getByRole('tab', { name: 'Mapping' }).click();
    await page.waitForTimeout(200);
    await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
      'light-mapping-default.png',
    );
  });
});

// Re-exported as named exports so the Task-2 matrix expansion can call
// them without re-declaring the helpers. Vitest-style sharing isn't
// available in Playwright (each spec file is hermetic) but co-locating
// the helpers + the matrix in this single spec is fine — the matrix is
// ~20 cells which keeps the file readable at one screen of scroll.
export { gotoLandingWithTheme, waitForReady, waitForViewerReady, disableAnimations, VIEWPORT };
