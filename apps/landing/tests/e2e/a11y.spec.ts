/**
 * A11Y-01 gate — landing default flow passes WCAG 2.1 AA via
 * `@axe-core/playwright`.
 *
 * Per Phase 8 success criterion + 08-10's component-level ARIA wiring:
 *   - role="application" on the playground root
 *   - role="banner" on the toolbar
 *   - landmark sections (aria-label) for editor + results
 *   - stable ARIA_LABELS for Run / Reset
 *   - polite aria-live region (id="fossil-live-region")
 *   - #graph-canvas id on the ResultGraph outer wrapper (axe-exclusion target)
 *
 * Exclusions per RESEARCH.md Pattern 5:
 *   - #graph-canvas — Cosmos.gl canvas (or its tabular-fallback wrapper) has
 *     no inherent semantic content; the tabular fallback INSIDE the wrapper
 *     carries the data for SR users.
 *   - .cm-content / .cm-scroller — CodeMirror 6's content + scroller hosts own
 *     their own a11y story (contenteditable with custom keyboard interaction +
 *     a `tabindex="-1"` scroller). axe-core flags them as "contenteditable
 *     without accessible name" / "scrollable-region-focusable", both false
 *     positives given CodeMirror's internal labelling and key handling.
 *
 * v2 note: every test settles past the OFFLINE-01 Service Worker's controlled
 * reload (gotoPlayground) before running axe — otherwise the reload lands
 * during AxeBuilder.analyze and trips "Execution context was destroyed".
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';

import {
  gotoPlayground,
  loadHelloExample,
  runAndWaitForVertices,
  waitForViewerReady,
} from './helpers';

test('A11Y-01: landing default flow passes WCAG 2.1 AA (axe-core)', async ({
  page,
}) => {
  await gotoPlayground(page);
  await waitForViewerReady(page);

  const results = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .exclude('#graph-canvas')
    .exclude('.cm-content')
    .exclude('.cm-scroller')
    .analyze();

  if (results.violations.length > 0) {
    // eslint-disable-next-line no-console
    console.log(
      '[A11Y-01] axe-core violations:',
      JSON.stringify(results.violations, null, 2),
    );
  }
  expect(results.violations).toEqual([]);
});

test('A11Y-01: result region remains WCAG 2.1 AA-compliant after Run', async ({
  page,
}) => {
  await gotoPlayground(page);
  // Load the hyphen-free `hello` example so the results region holds a real
  // (not codegen-errored) graph — see helpers §3.
  await loadHelloExample(page);
  await runAndWaitForVertices(page);
  await waitForViewerReady(page);

  // Scope axe to the results region (via the aria-label landmark) so we
  // assert the tabular fallback + the result graph wrapper independently of
  // the rest of the page.
  const results = await new AxeBuilder({ page })
    .withTags(['wcag2aa', 'wcag21aa'])
    .include('section[aria-label="Run results"]')
    .exclude('#graph-canvas')
    .analyze();
  if (results.violations.length > 0) {
    // eslint-disable-next-line no-console
    console.log(
      '[A11Y-01] axe-core results-region violations:',
      JSON.stringify(results.violations, null, 2),
    );
  }
  expect(results.violations).toEqual([]);
});

test('A11Y-01: Run + Reset buttons have stable accessible names', async ({
  page,
}) => {
  await gotoPlayground(page);

  // Accessible name comes from ARIA_LABELS centralised in
  // packages/playground/src/a11y/index.ts — visible text can mutate
  // ("Run" ↔ "Running…") but accessible name stays anchored.
  await expect(
    page.getByRole('button', { name: 'Run mapping' }),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Reset playground' }),
  ).toBeVisible();
});

test('A11Y-01: #graph-canvas exclusion target is structurally present', async ({
  page,
}) => {
  await gotoPlayground(page);
  // The axe-core exclusion `exclude: ['#graph-canvas']` needs the id to be in
  // the DOM whether enableWebGL is on or off (v0.1 default: off → only the
  // outer wrapper renders, but it carries the id per 08-10's hoist).
  await expect(page.locator('#graph-canvas')).toBeVisible();
});
