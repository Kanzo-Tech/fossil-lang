/**
 * PLAY-10 E2E gate — Turtle tab renders post-Run materialized triples
 * (post-Phase-14 "playground v2" FossilViewer tabs).
 *
 * v2 notes:
 *   - The result panel is the lazy-loaded FossilViewer, whose tablist exposes
 *     Graph / Turtle / Vertices (N) / Edges (N) — there is no "result views"
 *     accessible name on the tablist. We assert the tabs by their own names.
 *   - The default `hello-no-csvw` mapping trips the `derive_view_name` hyphen
 *     codegen bug, so the Run-dependent tests load the hyphen-free `hello`
 *     example first (helpers §3) and gate Run success on `Vertices (N>0)`
 *     (the IRIs themselves are on the non-selectable WebGL canvas).
 */
import { expect, test } from '@playwright/test';

import {
  gotoPlayground,
  loadHelloExample,
  runAndWaitForVertices,
  waitForViewerReady,
} from './helpers';

test('PLAY-10: clicking Run + Turtle tab shows Turtle text containing the IRIs', async ({
  page,
}) => {
  await gotoPlayground(page);
  await loadHelloExample(page);
  await runAndWaitForVertices(page);
  await waitForViewerReady(page);

  // Switch to the FossilViewer Turtle tab.
  await page.getByRole('tab', { name: /^turtle$/i }).click();
  const tab = page.getByTestId('turtle-tab');
  await expect(tab).toBeVisible();

  // Turtle should contain the `@prefix ex:` declaration block + the IRIs.
  // n3's Writer picks the shortest representation, so either the prefixed
  // CURIE form (`ex:user/1`) or the full IRI is acceptable.
  await expect(tab).toContainText(/@prefix\s+ex:/);
  await expect(tab).toContainText(/(ex:user|<https:\/\/example\.org\/user)/);
});

test('PLAY-10: Turtle tab has accessible Copy button', async ({ page }) => {
  await gotoPlayground(page);
  await loadHelloExample(page);
  await runAndWaitForVertices(page);
  await waitForViewerReady(page);

  await page.getByRole('tab', { name: /^turtle$/i }).click();

  const copyButton = page.getByTestId('turtle-copy');
  await expect(copyButton).toHaveAttribute(
    'aria-label',
    /copy turtle to clipboard/i,
  );
  await expect(copyButton).toBeVisible();
  // The button starts at "Copy" visible text; we don't exercise the clipboard
  // API (permissions policy differs across browsers) — accessible-name +
  // initial-text gates suffice for the PLAY-10 SC.
  await expect(copyButton).toHaveText(/copy/i);
});

test('PLAY-10: viewer tablist exposes Graph / Turtle / Vertices / Edges', async ({
  page,
}) => {
  await gotoPlayground(page);
  await waitForViewerReady(page);

  // The FossilViewer renders its tabs from page load (over zero rows) — no
  // Run required. The tab names are unique across the page's tablists so a
  // flat getByRole('tab', { name }) is unambiguous.
  await expect(page.getByTestId('fossil-viewer-root')).toBeVisible();
  await expect(page.getByRole('tab', { name: /^graph$/i })).toBeVisible();
  await expect(page.getByRole('tab', { name: /^turtle$/i })).toBeVisible();
  await expect(
    page.getByRole('tab', { name: /^Vertices \(\d+\)$/ }),
  ).toBeVisible();
  await expect(
    page.getByRole('tab', { name: /^Edges \(\d+\)$/ }),
  ).toBeVisible();
});
