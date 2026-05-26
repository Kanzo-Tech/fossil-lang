/**
 * ScrollArea primitive — unit tests. Covers render with overflowing content,
 * data-slot anatomy (root + viewport + scrollbar + thumb), default vertical
 * orientation, and ScrollBar named export overrideability.
 *
 * Per Phase 10 plan 10-04 Task 2.
 *
 * Note: happy-dom doesn't lay out content (no real scroll metrics), so we
 * don't assert on actual scroll behaviour — just on the DOM structure Radix
 * builds and the data attributes we drive styling off.
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { ScrollArea, ScrollBar } from '../src/primitives/ScrollArea.js';

describe('ScrollArea', () => {
  it('renders root + viewport + scrollbar with data-slot attributes (Thumb mounts on actual overflow, not asserted under happy-dom)', () => {
    render(
      <ScrollArea type="always" style={{ width: 200, height: 100 }}>
        <div style={{ height: 500 }}>Overflowing content</div>
      </ScrollArea>,
    );
    expect(document.querySelector('[data-slot="scroll-area"]')).not.toBeNull();
    expect(
      document.querySelector('[data-slot="scroll-area-viewport"]'),
    ).not.toBeNull();
    expect(
      document.querySelector('[data-slot="scroll-area-scrollbar"]'),
    ).not.toBeNull();
    // Thumb is mounted by Radix only when overflow is detected via
    // ResizeObserver + scrollHeight comparison; happy-dom doesn't run layout
    // so the Thumb stays unmounted. The real-browser fixture in plan 10-07
    // exercises the full Thumb mount path.
  });

  it('default scrollbar orientation is vertical', () => {
    render(
      <ScrollArea type="always" style={{ width: 200, height: 100 }}>
        <div style={{ height: 500 }}>Content</div>
      </ScrollArea>,
    );
    const bar = document.querySelector(
      '[data-slot="scroll-area-scrollbar"]',
    ) as HTMLElement;
    expect(bar.getAttribute('data-orientation')).toBe('vertical');
  });

  it('ScrollBar can be rendered horizontally via the named export', () => {
    render(
      <ScrollArea type="always" style={{ width: 200, height: 100 }}>
        <div style={{ width: 500 }}>Wide content</div>
        <ScrollBar orientation="horizontal" />
      </ScrollArea>,
    );
    const bars = document.querySelectorAll(
      '[data-slot="scroll-area-scrollbar"]',
    );
    const orientations = Array.from(bars).map((b) =>
      b.getAttribute('data-orientation'),
    );
    expect(orientations).toContain('horizontal');
    expect(orientations).toContain('vertical');
  });

  it('viewport inline style fills the root (width/height 100%)', () => {
    render(
      <ScrollArea type="always" style={{ width: 200, height: 100 }}>
        <div style={{ height: 500 }}>Content</div>
      </ScrollArea>,
    );
    const viewport = document.querySelector(
      '[data-slot="scroll-area-viewport"]',
    ) as HTMLElement;
    const style = viewport.getAttribute('style') || '';
    expect(style).toMatch(/width:\s*100%/);
    expect(style).toMatch(/height:\s*100%/);
  });
});
