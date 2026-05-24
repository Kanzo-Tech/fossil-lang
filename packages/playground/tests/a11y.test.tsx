/**
 * A11Y-01 unit tests — the COMPONENT-LEVEL ARIA wiring of <FossilPlayground/>.
 *
 * Scope:
 *   - Assert the structural ARIA attributes the apps/landing/ axe-core E2E
 *     gate (08-11) will exercise: role="application", landmark roles,
 *     accessible names on interactive controls, the `#graph-canvas` id that
 *     axe excludes per RESEARCH.md Pitfall 5.
 *   - Assert the `announce()` live region helper creates the canonical
 *     SR-only div + sets aria-live=polite.
 *
 * Not in scope (deferred to 08-11):
 *   - Full WCAG 2.1 AA gate via @axe-core/playwright against the mounted
 *     production bundle.
 *   - Keyboard-navigation traversal (Tab order, focus-trap inside CodeMirror).
 *     happy-dom doesn't compute focus deterministically enough for that.
 *   - Color contrast on the rendered editor surface (theme.test.tsx covers
 *     the palette-level assertion; the rendered contrast needs Playwright).
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import {
  FossilPlayground,
  ARIA_LABELS,
  announce,
  LIVE_REGION_ID,
} from '../src/index.js';
import { createMockResolver } from '@fossil-lang/resolvers';

const mockResolver = createMockResolver({
  fixtures: {},
  connectors: [{ name: 'examples', type: 'examples' as const }],
});

describe('A11Y-01: <FossilPlayground/> ARIA + roles', () => {
  it('the root has role="application" + an accessible name', () => {
    const { container } = render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    const root = container.querySelector('[role="application"]');
    expect(root).toBeTruthy();
    expect(root?.getAttribute('aria-label')).toBe('Fossil playground');
  });

  it('Run and Reset buttons carry the canonical ARIA labels', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    // getByRole queries the accessible name — proves the aria-label is being
    // surfaced correctly (vs the visible label which might differ during
    // loading states).
    expect(
      screen.getByRole('button', { name: ARIA_LABELS.runButton }),
    ).toBeTruthy();
    expect(
      screen.getByRole('button', { name: ARIA_LABELS.resetButton }),
    ).toBeTruthy();
  });

  it('the editor section is labelled for landmark navigation', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    // SR users navigate by landmark; a section with aria-label appears in
    // the landmark menu. The label MUST match the canonical constant so
    // future i18n can extract it.
    const editorRegion = screen.getByLabelText(ARIA_LABELS.editor);
    expect(editorRegion).toBeTruthy();
  });

  it('the results section is labelled for landmark navigation', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    const resultsRegion = screen.getByLabelText(ARIA_LABELS.resultsRegion);
    expect(resultsRegion).toBeTruthy();
  });

  it('ResultGraph wrapper carries the load-bearing #graph-canvas id', () => {
    // The id is the contract with the apps/landing/ axe-core gate (08-11),
    // which excludes `#graph-canvas` because WebGL canvases have no inherent
    // semantic content (RESEARCH.md Pitfall 5). The id MUST exist whether
    // or not enableWebGL is on — the tabular fallback path needs the same
    // landmark structure.
    const { container } = render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    const graphRegion = container.querySelector('#graph-canvas');
    expect(graphRegion).toBeTruthy();
    expect(graphRegion?.getAttribute('role')).toBe('img');
    expect(graphRegion?.getAttribute('aria-label')).toMatch(/tabular fallback/i);
  });

  it('the run-error alert region (when present) uses role="alert"', () => {
    // No error in the steady-state mount — the role="alert" element is
    // conditional. We verify the JSX renders no spurious alert at idle.
    const { container } = render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it('the toolbar uses role="banner" as a landmark', () => {
    render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    // banner is the toolbar at the top — landmark navigation surfaces it.
    expect(screen.getByRole('banner')).toBeTruthy();
  });
});

describe('A11Y-01: announce() live region', () => {
  beforeEach(() => {
    // Tear down any stale region from a prior test.
    const stale = document.getElementById(LIVE_REGION_ID);
    if (stale) stale.remove();
  });

  it('creates the SR-only live region on first call + sets aria-live=polite', () => {
    expect(document.getElementById(LIVE_REGION_ID)).toBeNull();
    const teardown = announce('Hello SR users');
    const region = document.getElementById(LIVE_REGION_ID);
    expect(region).toBeTruthy();
    expect(region!.getAttribute('aria-live')).toBe('polite');
    expect(region!.getAttribute('aria-atomic')).toBe('true');
    expect(region!.textContent).toBe('Hello SR users');
    // sr-only styling — the region must be visually hidden (1x1 + clipped)
    // but NOT display:none (which removes it from the accessibility tree).
    expect(region!.style.position).toBe('absolute');
    expect(region!.style.width).toBe('1px');
    expect(region!.style.height).toBe('1px');
    teardown();
    // Teardown clears textContent but leaves the region in the DOM for
    // reuse — one region per document.
    expect(region!.textContent).toBe('');
    expect(document.getElementById(LIVE_REGION_ID)).toBeTruthy();
  });

  it('reuses the same live region across multiple calls', () => {
    announce('First');
    const region1 = document.getElementById(LIVE_REGION_ID);
    announce('Second');
    const region2 = document.getElementById(LIVE_REGION_ID);
    expect(region1).toBe(region2);
    expect(region2!.textContent).toBe('Second');
  });

  it('returns a no-op teardown when document is undefined (SSR-safe)', () => {
    // Hold a reference to the real document, monkey-patch, restore.
    const realDocument = globalThis.document;
    // Trick TS: we deliberately undefine document for one assertion.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).document = undefined;
    try {
      const teardown = announce('SSR-impossible');
      expect(typeof teardown).toBe('function');
      // Calling teardown must not throw.
      expect(() => teardown()).not.toThrow();
    } finally {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (globalThis as any).document = realDocument;
    }
  });
});
