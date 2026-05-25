/**
 * PLAY-08 — BibTeX cite modal E2E gate.
 *
 * Asserts the modal mounts on the production landing build (`next start`):
 *   1. Cite trigger opens the modal; modal exposes both BibTeX sections.
 *   2. Min Oo & Hartig BibTeX is visible verbatim (cite-key + arXiv URL).
 *   3. With the editor's default mapping, the per-snapshot BibTeX section
 *      embeds a `playground.kanzo.dev/#<permalink>` URL.
 *   4. Pressing Escape closes the modal — exercises the native <dialog>
 *      Escape-handler the platform fires (not a JS keydown handler).
 *   5. The Copy-BibTeX + Copy-plaintext buttons carry disambiguating
 *      accessible names so SR users can target each one independently.
 *
 * Why on the LANDING build (not the multi-host fixture):
 *   The cite modal embeds the permalink from the FossilPlayground's
 *   onStateChange chain (PLAY-04, 09-05). The landing host wires
 *   window.location.hash to that chain, and the production deployment
 *   target referenced in the BibTeX URL is `playground.kanzo.dev` — so
 *   the prod-build landing is the canonical surface to verify the
 *   per-snapshot section against.
 */
import { expect, test } from '@playwright/test';

test('PLAY-08: Cite button opens modal with Hartig + playground BibTeX', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });

  // Wait for the permalink debounce (200 ms) so the editor's default
  // mapping has emitted a permalink that the modal can embed.
  await page.waitForTimeout(400);

  await page.getByTestId('cite-trigger').click();
  const modal = page.getByTestId('bibtex-modal');
  // Native <dialog> with the `open` attribute IS the platform's
  // "visible" state — toBeVisible exercises the same notion.
  await expect(modal).toBeVisible();

  // Hartig BibTeX content.
  await expect(modal).toContainText('minoo-hartig-2025-algebraic');
  await expect(modal).toContainText('2503.10385');
  await expect(modal).toContainText('Best Research Paper Award');

  // Per-snapshot BibTeX: cite-key + playground URL.
  await expect(modal).toContainText(/fossil-playground-[a-zA-Z0-9]+/);
  await expect(modal).toContainText(/playground\.kanzo\.dev\/#/);

  // Escape closes the dialog. Native <dialog> handles Escape itself; we
  // assert the React-side `open` state syncs (the close event listener
  // in BibtexModal flips it).
  await page.keyboard.press('Escape');
  await expect(modal).not.toBeVisible();
});

test('PLAY-08: Copy-BibTeX and Copy-plaintext buttons have accessible names', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  await page.waitForTimeout(400);
  await page.getByTestId('cite-trigger').click();

  // Each Copy button has a disambiguating aria-label so SR users can
  // distinguish "Copy Hartig BibTeX" from "Copy playground BibTeX".
  await expect(
    page.getByRole('button', { name: /copy hartig bibtex/i }),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: /copy hartig plaintext/i }),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: /copy playground bibtex/i }),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: /copy playground plaintext/i }),
  ).toBeVisible();
});

test('PLAY-08: modal close button closes the dialog (focus-trap proxy)', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByTestId('fossil-playground').waitFor({ timeout: 20_000 });
  await page.waitForTimeout(400);
  await page.getByTestId('cite-trigger').click();
  const modal = page.getByTestId('bibtex-modal');
  await expect(modal).toBeVisible();

  await page.getByTestId('bibtex-modal-close').click();
  await expect(modal).not.toBeVisible();
});
