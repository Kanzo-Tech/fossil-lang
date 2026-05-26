/**
 * Tooltip primitive — unit tests. Covers render with TooltipProvider wrap,
 * data-slot anatomy, ARIA tooltip role inheritance from Radix, focus surface
 * sentinel, and z-index inline style.
 *
 * Per Phase 10 plan 10-04 Task 2.
 *
 * Note: Radix Tooltip uses pointer/focus events + setTimeout(delayDuration)
 * to open the tooltip. happy-dom doesn't fire pointer events reliably, so
 * we use the controlled `open` prop on Tooltip Root to force the visible
 * state without depending on event timing.
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import {
  TooltipProvider,
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from '../src/primitives/Tooltip.js';

function renderTooltip(open: boolean) {
  return render(
    <TooltipProvider delayDuration={0}>
      <Tooltip open={open}>
        <TooltipTrigger>Hover me</TooltipTrigger>
        <TooltipContent>Helpful hint</TooltipContent>
      </Tooltip>
    </TooltipProvider>,
  );
}

describe('Tooltip', () => {
  it('renders trigger with data-slot when closed; no content in DOM', () => {
    renderTooltip(false);
    expect(
      document.querySelector('[data-slot="tooltip-trigger"]'),
    ).not.toBeNull();
    expect(
      document.querySelector('[data-slot="tooltip-content"]'),
    ).toBeNull();
  });

  it('exposes content with data-slot when open', () => {
    renderTooltip(true);
    const content = document.querySelector('[data-slot="tooltip-content"]');
    expect(content).not.toBeNull();
    // Radix renders BOTH the visible content + an aria-describedby sibling
    // span carrying the same text — so multiple text nodes match. Asserting
    // on the data-slot's textContent avoids the multiple-match collision.
    expect(content?.textContent).toContain('Helpful hint');
  });

  it('content has ARIA role="tooltip" inherited from Radix', () => {
    renderTooltip(true);
    // Radix renders an aria-describedby sibling for SR users PLUS the visible
    // popper. Both carry role="tooltip" — assert at least one exists.
    const tooltipsByRole = screen.getAllByRole('tooltip');
    expect(tooltipsByRole.length).toBeGreaterThanOrEqual(1);
  });

  it('content carries data-fossil-ui-primitive sentinel + z-index inline style', () => {
    renderTooltip(true);
    const content = document.querySelector(
      '[data-slot="tooltip-content"]',
    ) as HTMLElement;
    expect(content.hasAttribute('data-fossil-ui-primitive')).toBe(true);
    expect(content.getAttribute('style') || '').toMatch(/z-index/);
  });
});
