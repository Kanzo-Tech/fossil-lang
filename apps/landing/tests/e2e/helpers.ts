/**
 * Shared readiness helpers for the landing-playground e2e specs.
 *
 * These centralise three v2-playground invariants that every functional
 * spec has to respect — duplicating them per-file is how they drifted out
 * of sync in the first place (the 2026-05-25 specs predate the post-Phase-14
 * "playground v2" tabs refactor and the Phase-15 lazy-viewer + SW changes).
 *
 * 1. SERVICE-WORKER GUARD. The landing app registers a serwist Service
 *    Worker (OFFLINE-01) on mount which can fire a *controlled reload*
 *    mid-test. Without `waitForLoadState('networkidle')` after the initial
 *    navigation, that reload lands between a later `expect`/`evaluate` and
 *    its target, surfacing as "Execution context was destroyed, most likely
 *    because of a navigation" or a spurious permalink-hash navigation.
 *    `visual-baselines.spec.ts` already absorbs this the same way.
 *
 * 2. LAZY-VIEWER GATE. `@fossil-lang/viewer` is `React.lazy`'d inside the
 *    Output panel (Phase 15 plan 15-04). Until the chunk resolves the result
 *    region shows a "Loading viewer…" placeholder and the Graph/Turtle/
 *    Vertices/Edges tabs are absent.
 *
 * 3. DEFAULT-EXAMPLE CODEGEN BUG. The landing default mapping is
 *    `hello-no-csvw.fossil`; `fossil-codegen`'s `derive_view_name` emits
 *    `CREATE VIEW hello-no-csvw AS …`, which DuckDB rejects (unquoted
 *    identifiers cannot contain hyphens). Any spec that needs a *successful*
 *    Run loads the hyphen-free `hello` example first. This routes around the
 *    bug per `15-CONTEXT.md` ("NO toca compiler Rust"); the underlying
 *    codegen fix is tracked separately (it also breaks the landing default's
 *    own Run for real visitors).
 */
import { expect, type Page } from '@playwright/test';

/** Budget for "the Run completed at all" under 2-worker CI contention. */
export const RUN_BUDGET_MS = 15_000;

/**
 * Navigate to the landing playground and wait for a mountable steady state:
 * SW-controlled reload absorbed, dynamic-import resolved (testid visible),
 * and the WASM editor-mount gate cleared ("Loading editor…" gone).
 */
export async function gotoPlayground(page: Page): Promise<void> {
  await page.goto('/');
  // Absorb the OFFLINE-01 Service Worker's controlled reload before any
  // later evaluate/click can race it (see module docstring §1).
  await page.waitForLoadState('networkidle');
  await waitForReady(page);
}

/** Editor-mount gate: playground root visible + "Loading editor…" cleared. */
export async function waitForReady(page: Page): Promise<void> {
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });
}

/** Wait for the lazy-loaded FossilViewer chunk to mount (see docstring §2). */
export async function waitForViewerReady(page: Page): Promise<void> {
  await expect(page.getByText('Loading viewer…')).toHaveCount(0, {
    timeout: 10_000,
  });
}

/**
 * Swap the loaded example to the hyphen-free `hello` (see docstring §3) and
 * wait for the host remount to re-clear the editor-mount gate.
 */
export async function loadHelloExample(page: Page): Promise<void> {
  await page
    .getByRole('combobox', { name: 'Load curated example' })
    .selectOption('hello');
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 10_000,
  });
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();
}

/**
 * Click Run and wait for the success signal: the FossilViewer's
 * `Vertices (N>0)` tab label. The canonical IRIs live on the Cosmos.gl WebGL
 * canvas and are NOT text-selectable — the DOM-rendered tab counts are the
 * only reliable success surface. Returns the elapsed Run-to-success ms.
 */
export async function runAndWaitForVertices(page: Page): Promise<number> {
  const root = page.getByTestId('fossil-playground');
  const t0 = Date.now();
  await page.getByRole('button', { name: 'Run mapping' }).click();
  await expect(
    root.getByRole('tab', { name: /^Vertices \([1-9]\d*\)$/ }),
  ).toBeVisible({ timeout: RUN_BUDGET_MS });
  return Date.now() - t0;
}
