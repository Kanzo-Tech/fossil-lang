/**
 * Accessibility helpers — discharges the COMPONENT-LEVEL half of A11Y-01
 * (WCAG 2.1 AA). The full WCAG 2.1 AA E2E gate runs in `apps/landing/` via
 * `@axe-core/playwright` (08-11); this module provides the primitives that
 * gate exercises:
 *
 *   - `announce()`: appends transient messages to an SR-only `aria-live`
 *     region so the playground can describe async state to assistive tech
 *     (compiling/running/reset).
 *   - `ARIA_LABELS`: the canonical text strings used across the playground's
 *     interactive controls. Centralising keeps them consistent + extractable
 *     by a future i18n iteration (out of scope for v0.1 per CONTEXT.md
 *     non-functional requirements).
 *
 * Why a live region (vs e.g. updating button text):
 *   - Buttons that change label on each click confuse screen readers (the
 *     accessible name flips mid-interaction).
 *   - Toasts/snackbars rendered as siblings of the focused control are
 *     more discoverable than re-rendering the control itself.
 *   - The `polite` politeness setting batches updates and waits for the
 *     user to finish their current utterance — matches "Run complete"
 *     semantics (don't interrupt).
 *
 * The live region is created lazily on first use (per page) and reused —
 * one region per document is sufficient (multiple regions confuse SR
 * announcement ordering). SSR-safe (returns a no-op teardown if `document`
 * is undefined).
 */

/**
 * Stable id of the playground's single shared live region. The id is
 * load-bearing — tests + axe-core exclusions key on it; renaming is a
 * breaking change for downstream consumers that style the region.
 */
export const LIVE_REGION_ID = 'fossil-live-region';

/**
 * Append a transient message to the shared live region. Returns a teardown
 * that clears the region's text content so a stale message doesn't sit on
 * the page indefinitely.
 *
 * Politeness is `aria-live="polite"` (per WCAG 4.1.3 — Status Messages).
 * For ASSERTIVE alerts (e.g., compile errors), use `role="alert"` on a
 * separate inline element — see the runError block inside
 * `FossilPlayground.tsx`. `assertive` interrupts whatever the user is
 * currently hearing; we reserve it for genuine errors.
 *
 * SSR-safe: if `document` is undefined (server-side render), this is a
 * no-op returning a no-op teardown. The on-client first call lazily
 * creates the region.
 */
export function announce(
  message: string,
  regionId: string = LIVE_REGION_ID,
): () => void {
  if (typeof document === 'undefined') return noop;
  let region = document.getElementById(regionId);
  if (!region) {
    region = document.createElement('div');
    region.id = regionId;
    region.setAttribute('aria-live', 'polite');
    region.setAttribute('aria-atomic', 'true');
    // sr-only styling per WebAIM's canonical recipe — visually hidden
    // (no width/height/visible content) but reachable by screen readers
    // (NOT display:none, which removes the element from the accessibility
    // tree entirely).
    region.style.position = 'absolute';
    region.style.width = '1px';
    region.style.height = '1px';
    region.style.padding = '0';
    region.style.margin = '-1px';
    region.style.overflow = 'hidden';
    region.style.clip = 'rect(0,0,0,0)';
    region.style.whiteSpace = 'nowrap';
    region.style.border = '0';
    document.body.appendChild(region);
  }
  region.textContent = message;
  return () => {
    if (region) region.textContent = '';
  };
}

const noop = (): void => {};

/**
 * Canonical ARIA label strings for the playground's interactive controls.
 *
 * Each label is the ACCESSIBLE NAME for its control — what assistive tech
 * reads aloud when focus moves to it. Per WCAG 2.4.6 (Headings and Labels),
 * labels describe topic or purpose, not implementation. "Run mapping"
 * (purpose) is preferred over "Click to run" (instruction) per the
 * @testing-library/jest-dom and WAI-ARIA Authoring Practices guidance.
 *
 * Centralised here so:
 *   - The a11y test file (`tests/a11y.test.tsx`) can assert against the
 *     exact strings via `getByRole(..., { name: ARIA_LABELS.runButton })`.
 *   - A future i18n iteration extracts these from a single location.
 */
export const ARIA_LABELS = {
  /** Run button — primary action; the editor's mapping → DuckDB pipeline. */
  runButton: 'Run mapping',
  /** Reset button — per ADR-0026 this terminates the DuckDB Worker (NOT
   *  the LSP Worker; the LSP one stays warm). The label says
   *  "Reset playground" rather than "Reset DuckDB" because the user-facing
   *  semantics is "clear results"; the lifecycle asymmetry is an
   *  implementation detail. */
  resetButton: 'Reset playground',
  /** CodeMirror editor host. Note: the editor host's INNER contenteditable
   *  has its own SR semantics from CodeMirror — this label is on the
   *  WRAPPER section so the outer landmark navigation describes the
   *  region's purpose. */
  editor: 'Fossil mapping editor',
  /** The results section (vertex/edge tables + graph viz fallback). */
  resultsRegion: 'Run results',
  /** The WebGL graph canvas (when enableWebGL=true). Per RESEARCH.md
   *  Pitfall 5 + Pattern 4: axe-core excludes `#graph-canvas` via
   *  its id (canvases have no inherent semantic content; the tabular
   *  fallback below provides screen-reader-accessible data). */
  graphCanvas:
    'Result graph (WebGL); a tabular fallback follows below for screen readers',
} as const;
