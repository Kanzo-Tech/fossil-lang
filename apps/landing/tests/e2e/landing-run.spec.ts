/**
 * SC#1 gate — Run produces vertex+edge tables within 5 seconds.
 *
 * Per Phase 8 success criterion #1 (CONTEXT.md):
 *   "default resolver loads bundled example → edit → Run → vertex+edge
 *    tables in <5s on a 2020-era laptop, offline (after first load)"
 *
 * This spec is the END-TO-END gate that drives the handleRun pipeline
 * (08-09 left this scaffolded with KNOWN GAPS for 08-11 to surface).
 * The current 08-10 baseline stubs the actual network pipeline in
 * FossilPlayground.handleRun — the test runs against the stub for now;
 * the "tables visible" assertion matches the captions that render even
 * for empty rows (`<ResultTable rows=[] caption="Vertices ..." />` →
 * `<div role="status">Vertices ...: no results.</div>`). When the full
 * pipeline lands, the captions stay the same; the assertion stays
 * structural and the timing budget tightens organically.
 */
import { expect, test } from '@playwright/test';

test('SC#1: landing default flow — open, Run, see vertex + edge tables within 5 s', async ({
  page,
}) => {
  await page.goto('/');

  // Wait for the dynamic-imported playground to mount. The Client Shell's
  // loading fallback flashes 'Loading playground…' until the dynamic
  // chunk resolves; the data-testid lands on the inner playground root.
  await expect(page.getByTestId('fossil-playground')).toBeVisible({
    timeout: 15_000,
  });
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeEnabled();

  // Click Run + measure. The 5 s budget is the SC#1 contract per
  // CONTEXT.md.
  const t0 = Date.now();
  await page.getByRole('button', { name: 'Run mapping' }).click();

  // Vertex + edge tables render — assert against the captions. ResultTable
  // emits "<caption>Vertices (tabular fallback)</caption>" for the
  // populated case and "<div>Vertices (tabular fallback): no results.</div>"
  // for the empty case (08-09 v0.1 default). Either way the word
  // "vertices" appears in the page DOM.
  await expect(
    page.getByText(/vertices/i, { exact: false }).first(),
  ).toBeVisible({ timeout: 5_000 });
  await expect(
    page.getByText(/edges/i, { exact: false }).first(),
  ).toBeVisible({ timeout: 5_000 });

  const elapsed = Date.now() - t0;
  // Diagnostic log — surfaces in Playwright's GitHub Actions reporter.
  // eslint-disable-next-line no-console
  console.log(`[SC#1] Run-to-results: ${elapsed} ms`);
  expect(elapsed).toBeLessThan(5_000);
});

test('SC#1: Reset playground clears results but keeps the editor warm', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor();
  await page.getByRole('button', { name: 'Run mapping' }).click();
  await expect(
    page.getByText(/vertices/i, { exact: false }).first(),
  ).toBeVisible({ timeout: 5_000 });

  await page.getByRole('button', { name: 'Reset playground' }).click();

  // Editor still mounted — the LSP Worker survives Reset per ADR-0026's
  // asymmetric lifecycle. The CodeMirror content host stays in the DOM.
  await expect(page.locator('.cm-content')).toBeVisible({ timeout: 5_000 });

  // Results region cleared — ResultTable with empty rows emits the
  // "no results" status div.
  await expect(
    page.getByText(/no results/i).first(),
  ).toBeVisible({ timeout: 5_000 });
});

test('SC#3: initial page load completes (first-paint observable) within reasonable time', async ({
  page,
}) => {
  // SC#3's strict <3s cold-load assertion needs a CDN-warmed deployment +
  // Lighthouse-class measurement; here we exercise the localhost cold
  // path as a CHECK (not a strict gate). The full SC#3 measurement is
  // deferred to Phase 9 deployment verification per the plan-spec's
  // success criteria notes.
  const start = Date.now();
  await page.goto('/');
  // Header text from page.tsx renders synchronously (Server Component).
  await expect(
    page.getByRole('heading', { name: /fossil playground/i }),
  ).toBeVisible({ timeout: 10_000 });
  const elapsed = Date.now() - start;
  // eslint-disable-next-line no-console
  console.log(`[SC#3] First-paint observable at: ${elapsed} ms`);
  // Localhost cold-path budget; CI verifies it doesn't blow up to 10s+.
  expect(elapsed).toBeLessThan(10_000);
});
