/**
 * BibtexModal Vitest specs (PLAY-08).
 *
 * Covers (per 09-08 plan §3.1):
 *   1. Trigger button has an accessible name.
 *   2. Opening the modal reveals the Min Oo & Hartig BibTeX with the
 *      arXiv 2503.10385 URL embedded.
 *   3. With a non-empty permalink, the playground snapshot section
 *      shows the URL embedded into the @misc entry.
 *   4. With an undefined permalink, the snapshot section is hidden
 *      while the foundational-paper section is still visible.
 *   5. Pressing the Close button closes the modal — exercises the
 *      Close button path; Escape-close is exercised in Playwright
 *      where the platform's <dialog> focus trap actually fires.
 *
 * Why we don't rely on `getByTestId('bibtex-modal').toBeVisible()` —
 * happy-dom polyfills `<dialog>` but does NOT visually hide
 * non-`open` dialogs (CSS-driven), so a strict visibility check
 * would falsely pass. Instead we assert structural attributes:
 *   - The `open` attribute on <dialog> after `showModal()`.
 *   - The trigger's `aria-expanded` flips to "true" on open.
 */

import { describe, test, expect, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { BibtexModal } from '../src/bibtex/BibtexModal';

afterEach(cleanup);

describe('BibtexModal (PLAY-08)', () => {
  test('trigger button has an accessible name and aria-haspopup', () => {
    render(<BibtexModal permalink="abc123" />);
    const trigger = screen.getByTestId('cite-trigger');
    // happy-dom respects the aria-label attribute as the accessible
    // name when present (per ARIA's accessible-name computation).
    expect(trigger.getAttribute('aria-label')).toMatch(/citation modal/i);
    expect(trigger.getAttribute('aria-haspopup')).toBe('dialog');
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
  });

  test('opening the modal reveals the Min Oo & Hartig BibTeX', () => {
    render(<BibtexModal permalink="abc123" />);
    fireEvent.click(screen.getByTestId('cite-trigger'));

    // After showModal(), the <dialog> has the `open` attribute.
    const modal = screen.getByTestId('bibtex-modal');
    expect(modal.hasAttribute('open')).toBe(true);

    // Trigger's aria-expanded flips so SR users hear "expanded".
    const trigger = screen.getByTestId('cite-trigger');
    expect(trigger.getAttribute('aria-expanded')).toBe('true');

    const hartigBibtex = screen.getByTestId('hartig-bibtex');
    expect(hartigBibtex.textContent).toContain('minoo-hartig-2025-algebraic');
    expect(hartigBibtex.textContent).toContain('arxiv.org/abs/2503.10385');
    expect(hartigBibtex.textContent).toContain('Best Research Paper Award');
  });

  test('with non-empty permalink, the playground snapshot section shows', () => {
    render(<BibtexModal permalink="abcdef12345" />);
    fireEvent.click(screen.getByTestId('cite-trigger'));

    const playgroundBibtex = screen.getByTestId('playground-bibtex')
      .textContent ?? '';
    // The cite key uses the first 8 alphanumeric chars of the permalink.
    expect(playgroundBibtex).toContain('fossil-playground-abcdef12');
    // The URL embeds the permalink fragment so the cite round-trips state.
    expect(playgroundBibtex).toContain('playground.kanzo.dev/#abcdef12345');
    // The @misc keys are present.
    expect(playgroundBibtex).toContain('title  = {Fossil Playground');
    expect(playgroundBibtex).toContain('year   = {');
  });

  test('with undefined permalink, the snapshot section is hidden', () => {
    render(<BibtexModal permalink={undefined} />);
    fireEvent.click(screen.getByTestId('cite-trigger'));

    // Snapshot section is absent — only the foundational paper shows.
    expect(screen.queryByTestId('playground-bibtex')).toBeNull();
    // Hartig section still present.
    expect(screen.getByTestId('hartig-bibtex')).toBeTruthy();
  });

  test('Close button closes the dialog', () => {
    render(<BibtexModal permalink="xyz" />);
    fireEvent.click(screen.getByTestId('cite-trigger'));

    const modal = screen.getByTestId('bibtex-modal') as HTMLDialogElement;
    expect(modal.hasAttribute('open')).toBe(true);

    fireEvent.click(screen.getByTestId('bibtex-modal-close'));
    // After dlg.close(), the open attribute is removed AND the close
    // event fires on the dialog; the component listens to that event
    // and flips `open` state, which flips trigger's aria-expanded.
    expect(modal.hasAttribute('open')).toBe(false);
  });

  test('Copy buttons have disambiguating aria-labels', () => {
    render(<BibtexModal permalink="abcdef12345" />);
    fireEvent.click(screen.getByTestId('cite-trigger'));

    // Two Hartig copy buttons (BibTeX + plaintext).
    expect(
      screen.getByRole('button', { name: /copy hartig bibtex/i }),
    ).toBeTruthy();
    expect(
      screen.getByRole('button', { name: /copy hartig plaintext/i }),
    ).toBeTruthy();
    // Two playground copy buttons.
    expect(
      screen.getByRole('button', { name: /copy playground bibtex/i }),
    ).toBeTruthy();
    expect(
      screen.getByRole('button', { name: /copy playground plaintext/i }),
    ).toBeTruthy();
  });
});
