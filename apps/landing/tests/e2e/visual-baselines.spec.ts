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
 * # Matrix shape
 *
 * 2 themes × 5 tabs × 2 states = 20 cells. Each cell screenshots the
 * `<fossil-playground>` testid root (NOT page.screenshot() — that would
 * include the landing-page chrome + the ExampleSelector dropdown which
 * have their own dedicated specs and lifecycles).
 *
 * Tabs covered (per 15-CONTEXT.md): Mapping, Source, Shape (left panel)
 * + Output, Compiled SQL (right panel). The left and right panels are
 * sibling Radix Tabs widgets — switching the right-panel tab does NOT
 * change the active left-panel tab. The "default" cells screenshot the
 * playground with its default left-tab = Mapping + default right-tab =
 * Output; the per-tab "default" cells click the named tab on its panel
 * and screenshot the result.
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
  // Wait for Service Worker activation + any controlled reload to land.
  // Without this, downstream `page.evaluate()` and `selectOption()` calls
  // can trip "Execution context was destroyed, most likely because of a
  // navigation" when the SW activates mid-test (OFFLINE-01).
  await page.waitForLoadState('networkidle');
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
  // Wait for fonts via waitForFunction — auto-retries through any
  // mid-flight navigation (e.g. SW controlled reload, OFFLINE-01).
  // A bare `page.evaluate(() => document.fonts.ready)` trips
  // "Execution context was destroyed" if a navigation lands between
  // the previous gate and the evaluate call.
  await page.waitForFunction(
    () => document.fonts && document.fonts.status === 'loaded',
    null,
    { timeout: 5_000 },
  );
}

/**
 * Wait for the lazy-loaded viewer chunk (Cosmos.gl WebGL canvas) to
 * mount. Used by tests that screenshot the Output tab after a Run.
 *
 * Per Phase 15 plan 15-04 (commit c29679c), `@fossil-lang/viewer` is
 * React.lazy'd — the chunk fetch + Cosmos.gl init add ~200-500 ms
 * latency the first time the Output tab is mounted. The Suspense
 * fallback shows "Loading viewer…" until the chunk resolves; we wait
 * for it to vanish.
 */
async function waitForViewerReady(page: Page): Promise<void> {
  await expect(page.getByText('Loading viewer…')).toHaveCount(0, {
    timeout: 10_000,
  });
}

/**
 * Disable all CSS animations and transitions to capture stable frames.
 *
 * MUST be installed via `addInitScript` (BEFORE navigation) rather than
 * `addStyleTag` (AFTER navigation). The landing app registers a Service
 * Worker (OFFLINE-01) on mount which can trigger a controlled reload
 * mid-test — `addStyleTag` then trips "Execution context was destroyed,
 * most likely because of a navigation". `addInitScript` survives the
 * reload because Playwright re-injects it on every navigation.
 *
 * Inject as a `<style>` tag appended to `<head>` (before the page's own
 * stylesheets load → these rules win via `!important`).
 */
async function disableAnimations(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const css = `
      *, *::before, *::after {
        animation-duration: 0ms !important;
        animation-delay: 0ms !important;
        transition-duration: 0ms !important;
        transition-delay: 0ms !important;
        scroll-behavior: auto !important;
      }
    `;
    const inject = (): void => {
      if (!document.head) return;
      const style = document.createElement('style');
      style.setAttribute('data-baselines-injected', '1');
      style.textContent = css;
      document.head.appendChild(style);
    };
    if (document.head) {
      inject();
    } else {
      document.addEventListener('DOMContentLoaded', inject, { once: true });
    }
  });
}

/**
 * Run the mapping + wait for the success state.
 *
 * Mirrors run-twice.spec.ts's signal-flip pattern: click Run, wait for
 * the FossilViewer's `Vertices (N>0)` tab label inside the playground
 * root (the canonical IRIs sit on the Cosmos.gl WebGL canvas and are
 * NOT text-selectable — only the DOM-rendered tab counts surface the
 * success state).
 *
 * NOTE: the landing default is `hello-no-csvw.fossil` which (per
 * 15-01-SUMMARY's deferred-items.md) triggers a SEPARATE codegen bug
 * (derive_view_name returns `hello-no-csvw` which DuckDB rejects as
 * unquoted SQL identifier with hyphen). For this spec we load the
 * `hello` example explicitly via the ExampleSelector combobox so the
 * after-Run baselines reflect a clean Run. This mirrors run-twice.spec.ts.
 *
 * Precondition: the right-panel default tab is `output` (per
 * FossilPlayground.tsx — the default `activeRightTab = 'output'`), so
 * the Vertices/Edges tab labels are in the DOM from page load and stay
 * there regardless of which LEFT tab is active. Tests that screenshot
 * Source/Shape/Mapping AFTER a Run still get a valid success signal.
 */
async function loadHelloExampleAndRun(page: Page): Promise<void> {
  // Switch to the `hello` example (NOT the landing default
  // `hello-no-csvw`) — see docstring above for the codegen hyphen bug.
  // The ExampleSelector is a native <select> with aria-label="Load
  // curated example" per example-selector.tsx. Select by the manifest
  // option `value` (`'hello'`), which is more stable than the visible
  // label and avoids RegExp-vs-string Playwright API friction.
  await page
    .getByRole('combobox', { name: 'Load curated example' })
    .selectOption('hello');
  // Wait a beat for the host to remount the playground with the new
  // initialMapping. The remount tears down + rebuilds the editor; the
  // "Loading editor…" gate from waitForReady should re-clear.
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();
  await page.getByRole('button', { name: 'Run mapping' }).click();
  // Success gate: the FossilViewer's Vertices tab label flips from
  // `(0)` to `(N>0)`. The 5s SC#1 budget assumes a warm cold-start;
  // when this spec runs alongside 9 other after-Run cells under
  // Playwright's default fullyParallel=true, the WASM-Worker boot
  // contention pushes the 95th-percentile compile+run past 5s. Use
  // 15s as the budget here — we're not gating SC#1 performance,
  // we're gating "the Run succeeded at all" so the screenshot is
  // meaningful. A real SC#1 perf regression still trips landing-run.spec.ts
  // which is run independently.
  const root = page.getByTestId('fossil-playground');
  await expect(
    root.getByRole('tab', { name: /^Vertices \([1-9]\d*\)$/ }),
  ).toBeVisible({ timeout: 15_000 });
}

/**
 * Click a left-panel tab (Mapping/Source/Shape) and settle.
 *
 * Radix `<TabsTrigger/>` renders as `role="tab"` — both the left-panel
 * Tabs widget ("Input panels" aria-label) and the right-panel Tabs widget
 * ("Output panels" aria-label) are sibling DOM nodes. The tab names are
 * unique across both panels (Mapping/Source/Shape vs Output/Compiled SQL)
 * so a flat `getByRole('tab', { name: ... })` is unambiguous.
 */
async function selectTab(page: Page, name: string): Promise<void> {
  await page.getByRole('tab', { name }).click();
  await page.waitForTimeout(200); // settle for any tab-switch animation
}

/**
 * Pin the viewport to a stable size for all baselines. 1440 × 900 is a
 * common MacBook-class laptop resolution and is what was used to design
 * the post-Phase-14 IDE-tabs layout.
 */
const VIEWPORT = { width: 1440, height: 900 } as const;

type Theme = 'light' | 'dark';
const THEMES: readonly Theme[] = ['light', 'dark'] as const;

test.describe('Visual baselines — playground v2 (post-Phase-14)', () => {
  // Serial mode: WASM-Worker contention under Playwright's default
  // fullyParallel=true pushes the 95th-percentile after-Run compile past
  // even a generous 15 s budget on some cells (the LSP Worker boot races
  // with the DuckDB-WASM Worker boot races with the FossilWasm Worker
  // boot — 9 concurrent cells means up to 27 cold Workers competing).
  // Serial execution adds ~3 min wall-clock to a CI run but makes the
  // gate REPRODUCIBLE, which is the entire point of the visual baseline.
  // Phase 17 (REL-03) cross-host suite is the place to optimise CI
  // duration; for now correctness > speed.
  test.describe.configure({ mode: 'serial' });

  test.beforeEach(async ({ page }) => {
    await page.setViewportSize(VIEWPORT);
    // disableAnimations uses addInitScript, which MUST be installed
    // BEFORE the first navigation. Hoisting it into beforeEach (instead
    // of calling it after gotoLandingWithTheme) lets us be hermetic
    // against Service Worker controlled reloads which would otherwise
    // wipe a post-navigation addStyleTag injection (OFFLINE-01).
    await disableAnimations(page);
  });

  // ---------- DEFAULT-STATE MATRIX (no Run clicked) ----------
  //
  // 2 themes × 5 tabs = 10 cells. Each test loads the landing page in
  // the chosen theme, waits for the editor mount gate, clicks the named
  // tab, and screenshots the `<fossil-playground>` testid root.

  for (const theme of THEMES) {
    test(`SC#4: baseline — ${theme} theme, Mapping tab, default state`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await selectTab(page, 'Mapping');
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-mapping-default.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Source tab, default state`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await selectTab(page, 'Source');
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-source-default.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Shape tab, default state`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await selectTab(page, 'Shape');
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-shape-default.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Output tab, default state`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await selectTab(page, 'Output');
      // No Run yet — Output renders its "no results" placeholder. The
      // lazy viewer chunk MAY still mount the empty-state shell; wait
      // for the "Loading viewer…" fallback to clear before screenshot.
      await waitForViewerReady(page);
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-output-default.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Compiled SQL tab, default state`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await selectTab(page, 'Compiled SQL');
      // Compiled SQL panel is debounced (PLAY-07 200 ms) — give it
      // enough time to compute the initial SQL.
      await page.waitForTimeout(400);
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-compiled-sql-default.png`,
      );
    });
  }

  // ---------- AFTER-RUN MATRIX (Run clicked, hello example) ----------
  //
  // 2 themes × 5 tabs = 10 cells. Each test loads the landing page in
  // the chosen theme, switches to the `hello` example (avoids the
  // hello-no-csvw codegen hyphen bug; see loadHelloExampleAndRun docstring),
  // clicks Run, waits for real-IRI success, then switches to the named
  // tab and screenshots.
  //
  // Output tab after-Run is the FLAKINESS RISK: the Cosmos.gl WebGL
  // canvas renders pixel-noise inherent to GPU-driven layout. The 2%
  // tolerance should absorb it, but if it doesn't we mark the cell
  // `test.fixme` with a comment.

  for (const theme of THEMES) {
    test(`SC#4: baseline — ${theme} theme, Mapping tab, after Run`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await loadHelloExampleAndRun(page);
      await selectTab(page, 'Mapping');
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-mapping-after-run.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Source tab, after Run`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await loadHelloExampleAndRun(page);
      await selectTab(page, 'Source');
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-source-after-run.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Shape tab, after Run`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await loadHelloExampleAndRun(page);
      await selectTab(page, 'Shape');
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-shape-after-run.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Output tab, after Run`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await loadHelloExampleAndRun(page);
      await selectTab(page, 'Output');
      // WebGL canvas needs extra time to settle after the data update.
      // Cosmos.gl runs an internal simulation tick for graph layout; the
      // canvas is "stable enough" for screenshotting after ~500 ms.
      await waitForViewerReady(page);
      await page.waitForTimeout(500);
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-output-after-run.png`,
      );
    });

    test(`SC#4: baseline — ${theme} theme, Compiled SQL tab, after Run`, async ({
      page,
    }) => {
      await gotoLandingWithTheme(page, theme);
      await waitForReady(page);
      await loadHelloExampleAndRun(page);
      await selectTab(page, 'Compiled SQL');
      // PLAY-07 200 ms debounce + recompile after example-switch.
      await page.waitForTimeout(400);
      await expect(page.getByTestId('fossil-playground')).toHaveScreenshot(
        `${theme}-compiled-sql-after-run.png`,
      );
    });
  }
});

// Re-exported as named exports so future specs (e.g. Phase 17 REL-03
// cross-host suite) can import and reuse the helpers without redeclaring.
export {
  gotoLandingWithTheme,
  waitForReady,
  waitForViewerReady,
  disableAnimations,
  loadHelloExampleAndRun,
  selectTab,
  VIEWPORT,
};
