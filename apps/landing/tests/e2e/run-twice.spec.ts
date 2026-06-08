/**
 * BUG-01 regression canary — Run twice in a row must produce IDENTICAL results.
 *
 * Why this spec exists (Phase 15 plan 15-01 + REQUIREMENTS.md BUG-01):
 *   v0.1 user report (2026-05-26): clicking Run TWICE in a row did NOT
 *   produce two equivalent result sets (Salsa store accumulation, DuckDB
 *   connection-state leak, or a stale `compileInstance` revision). After the
 *   15-01 fix the second Run reproduces the first exactly.
 *
 * What this spec asserts:
 *   1. Load the hyphen-free `hello` example (the landing default
 *      `hello-no-csvw` trips the independent `derive_view_name` hyphen
 *      codegen bug — see helpers §3).
 *   2. Run. Wait for the FossilViewer `Vertices (N>0)` tab label. Capture the
 *      first-Run vertex + edge counts.
 *   3. Run AGAIN. Wait for the count to repopulate.
 *   4. Assert: same counts both times, no `role="alert"` error after either.
 *
 * Diagnosis aid — if this FAILS in CI: read
 * `await page.locator('[role="alert"]').textContent()` for the concrete error
 * (DuckDB binding → runPipeline virtual-FS registration; Salsa "stale
 * revision" / duplicate `registerInferredDescriptor` → introspectAndRegister).
 * The vitest twin lives at
 * `packages/playground/tests/runPipeline.run-twice.test.ts`.
 */
import { expect, test } from '@playwright/test';

import {
  gotoPlayground,
  loadHelloExample,
  waitForViewerReady,
} from './helpers';

test('BUG-01: clicking Run twice in a row produces identical results (no state-leak)', async ({
  page,
}) => {
  await gotoPlayground(page);
  await loadHelloExample(page);
  await waitForViewerReady(page);

  const root = page.getByTestId('fossil-playground');
  // The post-Run vertex/edge counts surface in the FossilViewer tab labels
  // (`Vertices (N)` / `Edges (N)`) — these live in the DOM (unlike the IRIs
  // themselves, which sit on the non-selectable WebGL canvas). They ARE the
  // success signal: on success the labels flip from `(0)` to `(N>0)`; on
  // BUG-01 they stay `(0)` and a `role="alert"` banner appears.
  const verticesTabLabel = (): Promise<string | null> =>
    root.getByRole('tab', { name: /^Vertices \(\d+\)$/ }).textContent();
  const edgesTabLabel = (): Promise<string | null> =>
    root.getByRole('tab', { name: /^Edges \(\d+\)$/ }).textContent();

  // Error banner is a visible `role="alert"` inside the playground root. The
  // page-level live-region also surfaces a stray empty "alert", so scope to
  // the playground tree.
  const playgroundAlerts = root.locator('[role="alert"]');

  // ----- First Run -----
  await page.getByRole('button', { name: 'Run mapping' }).click();
  await expect(
    root.getByRole('tab', { name: /^Vertices \([1-9]\d*\)$/ }),
  ).toBeVisible({ timeout: 15_000 });

  const firstRunVertices = await verticesTabLabel();
  const firstRunEdges = await edgesTabLabel();
  expect(firstRunVertices).toMatch(/Vertices \([1-9]\d*\)/); // sanity
  await expect(playgroundAlerts).toHaveCount(0, { timeout: 1_000 });

  // ----- Second Run -----
  // handleRun zeroes vertices before awaiting runPipeline, so the labels may
  // flicker back to `(0)`; poll for the post-second-Run non-zero count.
  await page.getByRole('button', { name: 'Run mapping' }).click();
  await expect(
    root.getByRole('tab', { name: /^Vertices \([1-9]\d*\)$/ }),
  ).toBeVisible({ timeout: 15_000 });

  const secondRunVertices = await verticesTabLabel();
  const secondRunEdges = await edgesTabLabel();

  // The core BUG-01 assertion: same counts both times.
  expect(secondRunVertices).toBe(firstRunVertices);
  expect(secondRunEdges).toBe(firstRunEdges);
  await expect(playgroundAlerts).toHaveCount(0, { timeout: 1_000 });
});
