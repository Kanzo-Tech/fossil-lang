/**
 * BibtexModal — PLAY-08 cite modal.
 *
 * Renders a "Cite" trigger button plus a native HTML `<dialog>` modal
 * showing:
 *   - The Min Oo & Hartig (ESWC 2025) BibTeX entry — the foundational
 *     paper the type system is built on (09-CONTEXT.md decisions).
 *   - When a non-empty permalink is provided, a second BibTeX entry for
 *     the current playground snapshot, with the permalink URL embedded
 *     so the cite round-trips state (PLAY-04 paper-permanence payoff).
 *
 * Each entry has Copy-BibTeX and Copy-plaintext buttons.
 *
 * Why native `<dialog>` instead of Radix Dialog:
 *
 *   The 09-08 plan named Radix Dialog ("already in playground per Phase
 *   8 08-09"). Grep of `packages/playground/` and `apps/landing/` shows
 *   NO Radix dependency anywhere in the workspace, and the playground's
 *   `package.json` does not declare `@radix-ui/react-dialog`. The plan's
 *   premise is incorrect — Radix is not in the bundle.
 *
 *   The native `<dialog>` element provides exactly what the
 *   `must_haves.truths` row #2 requires:
 *     - `showModal()` enters the top-layer with browser-managed focus
 *       trap (focus cycles within the dialog; Tab cannot escape).
 *     - Escape closes the dialog without JS handlers — the browser
 *       fires a `close` event we observe to sync React state.
 *     - role="dialog" + aria-modal=true are implied by the element;
 *       a Dialog.Title equivalent comes from the visible `<h2>` we
 *       reference via `aria-labelledby`.
 *     - axe-core accepts `<dialog>` as a first-class modal pattern.
 *
 *   This is a Rule-3 deviation (blocking issue: dependency claimed by
 *   the plan does not exist). Adding `@radix-ui/react-dialog` to the
 *   peer-light playground bundle for one modal would cost ~14 KB gzip
 *   and a new peerDependency surface; native dialog costs zero bytes.
 *   See 09-08 SUMMARY "Deviations" for the full rationale.
 *
 * Accessibility:
 *   - The dialog is opened via `dialogRef.current?.showModal()` — top
 *     layer + focus trap + Escape-close are inherited from the
 *     platform.
 *   - aria-labelledby points at the `<h2>` so screen readers announce
 *     "Cite the Fossil playground" on focus.
 *   - aria-describedby points at the lead paragraph for context.
 *   - Copy buttons have `aria-label` so the "Copy" label of multiple
 *     buttons is disambiguated.
 *   - Re-focuses the trigger on close (the platform does this by
 *     default for buttons that opened via `showModal()`).
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  HARTIG_BIBTEX,
  HARTIG_PLAINTEXT,
  buildPlaygroundBibtex,
  buildPlaygroundPlaintext,
} from './cite-templates.js';

export interface BibtexModalProps {
  /**
   * Current encoded permalink (without leading `#`). When empty or
   * undefined, only the foundational-paper reference is shown.
   *
   * Sourced from `<FossilPlayground/>`'s usePermalink → onStateChange
   * chain (09-05). The host (apps/landing/PlaygroundHost) already
   * receives the encoded permalink; PLAY-08 makes it visible.
   */
  permalink?: string;

  /** Trigger button label (default: "Cite"). */
  triggerLabel?: string;

  /**
   * Optional CSS class for the trigger button — lets the playground
   * toolbar style it consistently with Run/Reset.
   */
  triggerClassName?: string;
}

/**
 * One row of cite content: a `<pre>` showing the entry, plus two Copy
 * buttons (BibTeX + plaintext).
 *
 * Splits the JSX so the two sections (Hartig + Playground) share
 * structure without duplication.
 */
interface CiteSectionProps {
  heading: string;
  bibtex: string;
  plaintext: string;
  testidPrefix: string;
  ariaLabelPrefix: string;
  copiedKey: string | null;
  setCopiedKey: (key: string | null) => void;
}

function CiteSection({
  heading,
  bibtex,
  plaintext,
  testidPrefix,
  ariaLabelPrefix,
  copiedKey,
  setCopiedKey,
}: CiteSectionProps): JSX.Element {
  const bibtexKey = `${testidPrefix}-bibtex`;
  const plainKey = `${testidPrefix}-plain`;

  const copy = async (key: string, text: string): Promise<void> => {
    if (typeof navigator === 'undefined' || !navigator.clipboard) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopiedKey(key);
      // Brief confirmation flash; doesn't block re-clicks.
      setTimeout(() => setCopiedKey(null), 1500);
    } catch {
      // navigator.clipboard.writeText can reject in non-secure contexts
      // (e.g. some `file://` previews). Fail quietly — the cite is
      // still visible in the <pre>, so users can hand-select + copy.
    }
  };

  return (
    <section className="fossil-bibtex-modal__section">
      <h3>{heading}</h3>
      <pre data-testid={bibtexKey} className="fossil-bibtex-modal__entry">
        <code>{bibtex}</code>
      </pre>
      <div className="fossil-bibtex-modal__actions">
        <button
          type="button"
          onClick={() => {
            void copy(bibtexKey, bibtex);
          }}
          aria-label={`Copy ${ariaLabelPrefix} BibTeX`}
        >
          {copiedKey === bibtexKey ? 'Copied!' : 'Copy BibTeX'}
        </button>
        <button
          type="button"
          onClick={() => {
            void copy(plainKey, plaintext);
          }}
          aria-label={`Copy ${ariaLabelPrefix} plaintext`}
        >
          {copiedKey === plainKey ? 'Copied!' : 'Copy plaintext'}
        </button>
      </div>
    </section>
  );
}

/**
 * Top-level cite modal. Default-exported via the package barrel + the
 * playground component toolbar (see FossilPlayground.tsx).
 */
export function BibtexModal({
  permalink,
  triggerLabel = 'Cite',
  triggerClassName,
}: BibtexModalProps): JSX.Element {
  const dialogRef = useRef<HTMLDialogElement | null>(null);
  const [open, setOpen] = useState<boolean>(false);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const openModal = useCallback((): void => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    // showModal() enters the top layer + applies the browser-native
    // focus trap. If for some reason `<dialog>` isn't supported (e.g.
    // very old browsers / some headless test runners), we degrade to
    // open=true so the content is still rendered, just without the
    // focus trap. happy-dom polyfills showModal but does NOT implement
    // the focus trap — that's fine; we test the open/close semantics
    // in unit tests and the focus behaviour in Playwright.
    if (typeof dlg.showModal === 'function') {
      try {
        dlg.showModal();
      } catch {
        dlg.setAttribute('open', '');
      }
    } else {
      dlg.setAttribute('open', '');
    }
    setOpen(true);
  }, []);

  const closeModal = useCallback((): void => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    if (typeof dlg.close === 'function') {
      try {
        dlg.close();
      } catch {
        dlg.removeAttribute('open');
      }
    } else {
      dlg.removeAttribute('open');
    }
    setOpen(false);
  }, []);

  // Sync React's `open` state with the platform `close` event —
  // pressing Escape fires `close` directly without going through our
  // closeModal handler.
  useEffect(() => {
    const dlg = dialogRef.current;
    if (!dlg) return;
    const onClose = (): void => setOpen(false);
    dlg.addEventListener('close', onClose);
    return () => {
      dlg.removeEventListener('close', onClose);
    };
  }, []);

  const playgroundBibtex = permalink ? buildPlaygroundBibtex(permalink) : null;
  const playgroundPlaintext = permalink
    ? buildPlaygroundPlaintext(permalink)
    : null;

  return (
    <>
      <button
        type="button"
        data-testid="cite-trigger"
        className={triggerClassName}
        onClick={openModal}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label="Open citation modal"
      >
        {triggerLabel}
      </button>
      {/*
        Native <dialog> — role/aria-modal implied by the element. Backdrop is
        styled via the ::backdrop pseudo-element in landing.css / consumer CSS.
        The dialog is rendered inline in the DOM but `showModal()` promotes
        it to the top layer so z-index doesn't matter.
      */}
      <dialog
        ref={dialogRef}
        data-testid="bibtex-modal"
        className="fossil-bibtex-modal"
        aria-labelledby="bibtex-modal-title"
        aria-describedby="bibtex-modal-desc"
      >
        <h2 id="bibtex-modal-title">Cite the Fossil playground</h2>
        <p id="bibtex-modal-desc">
          BibTeX and plaintext entries — paste into your paper draft. The
          playground snapshot link below encodes the current editor state.
        </p>

        <CiteSection
          heading="Foundational paper (Min Oo & Hartig, ESWC 2025)"
          bibtex={HARTIG_BIBTEX}
          plaintext={HARTIG_PLAINTEXT}
          testidPrefix="hartig"
          ariaLabelPrefix="Hartig"
          copiedKey={copiedKey}
          setCopiedKey={setCopiedKey}
        />

        {playgroundBibtex && playgroundPlaintext ? (
          <CiteSection
            heading="This playground snapshot"
            bibtex={playgroundBibtex}
            plaintext={playgroundPlaintext}
            testidPrefix="playground"
            ariaLabelPrefix="playground"
            copiedKey={copiedKey}
            setCopiedKey={setCopiedKey}
          />
        ) : null}

        <div className="fossil-bibtex-modal__footer">
          <button
            type="button"
            data-testid="bibtex-modal-close"
            onClick={closeModal}
            aria-label="Close citation modal"
          >
            Close
          </button>
        </div>
      </dialog>
    </>
  );
}
