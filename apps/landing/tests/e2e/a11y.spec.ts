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
 *   - #graph-canvas id on the ResultGraph outer wrapper (axe-exclusion
 *     target — RESEARCH.md Pattern 5 + Pitfall 5)
 *
 * Exclusions per RESEARCH.md Pattern 5:
 *   - #graph-canvas — Cosmos.gl canvas (or its tabular-fallback wrapper)
 *     has no inherent semantic content; the tabular fallback INSIDE the
 *     wrapper carries the data for SR users.
 *   - .cm-content — CodeMirror 6's content host owns its own a11y story
 *     (it's a contenteditable with custom keyboard interaction); axe-core
 *     flags it as "missing form label" / "contenteditable without
 *     accessible name" which is a false positive given CodeMirror's
 *     internal labelling.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';

test('A11Y-01: landing default flow passes WCAG 2.1 AA (axe-core)', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 15_000 });

  const results = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .exclude('#graph-canvas')
    .exclude('.cm-content')
    .analyze();

  // Surface any violations in the test output for debugging.
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
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 15_000 });
  await page.getByRole('button', { name: 'Run mapping' }).click();
  await expect(
    page.getByText(/vertices/i, { exact: false }).first(),
  ).toBeVisible({ timeout: 5_000 });

  // Scope axe to the results region (via the aria-label landmark) so we
  // assert the tabular fallback + the result graph wrapper independently
  // of the rest of the page.
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
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 15_000 });

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
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 15_000 });
  // The axe-core exclusion `exclude: ['#graph-canvas']` needs the id to be
  // in the DOM whether enableWebGL is on or off (v0.1 default: off → only
  // the outer wrapper renders, but it carries the id per 08-10's hoist).
  await expect(page.locator('#graph-canvas')).toBeVisible();
});
