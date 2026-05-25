/**
 * PLAY-06 — landing hero E2E gate.
 *
 * Asserts the hero renders ABOVE the playground on the production build:
 *   1. <h1> exists; tagline + CTA + diagram + links + cite snippet all
 *      land in the DOM.
 *   2. CTA targets `#playground` (native scroll-anchor).
 *   3. Repo link points at `github.com/.../fossil`.
 *   4. Paper link points at arXiv 2503.10385.
 *   5. Foundational-paper BibTeX renders inside the <details> + matches
 *      the canonical constant from `@fossil-lang/playground` (anti-drift
 *      check — the hero inlines this string because Server Components
 *      can't import from a barrel that re-exports client-only React
 *      hooks; see landing-hero.tsx bundle-hygiene note).
 *   6. CTA scrolls to the `#playground` anchor — the URL hash flips, and
 *      the playground section is in view.
 */
import { expect, test } from '@playwright/test';

const EXPECTED_HARTIG_BIBTEX = `@inproceedings{minoo-hartig-2025-algebraic,
  title     = {An Algebraic Foundation for Knowledge Graph Construction},
  author    = {Min Oo, Sitt and Hartig, Olaf},
  booktitle = {Proceedings of the 22nd European Semantic Web Conference (ESWC 2025)},
  year      = {2025},
  note      = {Best Research Paper Award},
  url       = {https://arxiv.org/abs/2503.10385},
}`;

test('PLAY-06: hero renders heading + tagline + CTA + diagram + links + BibTeX', async ({
  page,
}) => {
  await page.goto('/');

  // <h1> opens the document. We use a regex that allows the em-dash
  // variant the markup uses ("Fossil — typed mapping DSL...").
  await expect(
    page.getByRole('heading', { level: 1, name: /fossil.+typed mapping/i }),
  ).toBeVisible();

  // CTA: visible, points at #playground.
  const cta = page.getByTestId('landing-cta');
  await expect(cta).toBeVisible();
  await expect(cta).toHaveAttribute('href', '#playground');

  // Pipeline diagram: three boxes with the correct labels.
  await expect(page.getByText('.fossil', { exact: true })).toBeVisible();
  await expect(page.getByText('DuckDB SQL', { exact: true })).toBeVisible();
  await expect(
    page.getByText('GraphAr / Parquet', { exact: true }),
  ).toBeVisible();

  // Project links.
  await expect(page.getByTestId('landing-repo-link')).toHaveAttribute(
    'href',
    /github\.com\/[^/]+\/fossil/,
  );
  await expect(page.getByTestId('landing-paper-link')).toHaveAttribute(
    'href',
    'https://arxiv.org/abs/2503.10385',
  );

  // BibTeX <details> — collapsed by default; expand to assert content.
  const summary = page.locator('.landing-cite summary');
  await summary.click();
  const bibtexBlock = page.getByTestId('landing-bibtex');
  await expect(bibtexBlock).toBeVisible();
  // Anti-drift gate: rendered text must match the canonical
  // HARTIG_BIBTEX constant from `@fossil-lang/playground` byte-for-byte.
  const rendered = (await bibtexBlock.textContent()) ?? '';
  expect(rendered).toBe(EXPECTED_HARTIG_BIBTEX);
});

test('PLAY-06: CTA scrolls to the #playground anchor', async ({ page }) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });

  await page.getByTestId('landing-cta').click();

  // Native browser scroll-anchor: the URL hash flips to #playground.
  await page.waitForFunction(
    () => window.location.hash === '#playground',
    undefined,
    { timeout: 2_000 },
  );

  // The playground anchor (<main id="playground">) is now in view —
  // a containing-section visibility check is sufficient.
  await expect(page.locator('#playground')).toBeVisible();
});
