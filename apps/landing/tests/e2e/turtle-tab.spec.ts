/**
 * PLAY-10 E2E gate — Turtle tab renders post-Run materialized triples.
 *
 * Per 09-CONTEXT.md locked decision:
 *   - Tab alongside Graph + Edges in the result panel.
 *   - Post-Run: vertex/edge tables → rowsToTurtle (n3-backed; from 09-04).
 *   - Selectable + copyable; Copy button with accessible label.
 *   - Inherits FossilTheme.
 *
 * Depends on 09-01 (CODEGEN-LOWERING-01 fix) — without it the post-Run
 * vertex IRIs never materialise, so this spec would fail on the Run-completion
 * gate before reaching the Turtle assertion.
 */
import { expect, test } from '@playwright/test';

test('PLAY-10: clicking Run + Turtle tab shows Turtle text containing the IRIs', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 15_000,
  });

  // Click Run — wait for vertex/edge tables to populate (the SC#1 contract
  // from landing-run.spec.ts; 5s budget is the tight gate).
  await page.getByRole('button', { name: 'Run mapping' }).click();
  const root = page.getByTestId('fossil-playground');
  await expect(
    root.getByText(/https:\/\/example\.org\/user\/[0-9]+/).first(),
  ).toBeVisible({ timeout: 10_000 });

  // Switch to Turtle tab.
  await page.getByRole('tab', { name: /^turtle$/i }).click();
  const tab = page.getByTestId('turtle-tab');
  await expect(tab).toBeVisible();

  // Turtle should contain the IRIs + @prefix declarations. n3 emits the
  // `@prefix ex: <https://example.org/> .` declaration block at the top
  // (the TURTLE_DEFAULT_PREFIXES map registers `ex` to the example.org
  // namespace). The IRIs themselves shorten to `ex:user/1` form after
  // prefix application.
  await expect(tab).toContainText(/@prefix\s+ex:/);
  // Either the unshortened IRI OR the prefixed CURIE form is acceptable —
  // n3's Writer picks the shortest representation given the prefix table.
  await expect(tab).toContainText(/(ex:user|<https:\/\/example\.org\/user)/);
});

test('PLAY-10: Turtle tab has accessible Copy button', async ({ page }) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 15_000,
  });

  await page.getByRole('button', { name: 'Run mapping' }).click();
  const root = page.getByTestId('fossil-playground');
  await expect(
    root.getByText(/https:\/\/example\.org\/user\/[0-9]+/).first(),
  ).toBeVisible({ timeout: 10_000 });

  await page.getByRole('tab', { name: /^turtle$/i }).click();

  const copyButton = page.getByTestId('turtle-copy');
  await expect(copyButton).toHaveAttribute(
    'aria-label',
    /copy turtle to clipboard/i,
  );
  await expect(copyButton).toBeVisible();
  // The button starts at "Copy" visible text; clicking it flips to "Copied!"
  // for 1.5s. We can't reliably exercise the clipboard API in CI (permissions
  // policy differs across browsers), but the accessible-name + initial-text
  // gates suffice for the PLAY-10 SC.
  await expect(copyButton).toHaveText(/copy/i);
});

test('PLAY-10: tablist exposes role="tab" for Graph / Edges / Turtle', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  await expect(page.getByText('Loading editor…')).toHaveCount(0, {
    timeout: 15_000,
  });

  // Tablist landmark + three tabs visible without needing a Run first
  // (the tablist exists pre-Run; the Turtle panel just shows an empty
  // serialization until vertices/edges populate).
  const tablist = page.getByRole('tablist', { name: /result views/i });
  await expect(tablist).toBeVisible();
  await expect(page.getByRole('tab', { name: /^graph$/i })).toBeVisible();
  await expect(page.getByRole('tab', { name: /^edges$/i })).toBeVisible();
  await expect(page.getByRole('tab', { name: /^turtle$/i })).toBeVisible();
});
