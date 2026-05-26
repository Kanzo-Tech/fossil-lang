/**
 * Separator primitive — unit tests. Covers render, data-slot, orientation,
 * token consumption, and ARIA semantics (decorative vs semantic).
 *
 * Per Phase 10 plan 10-03 Task 2.
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Separator } from '../src/primitives/Separator.js';

describe('Separator', () => {
  it('renders with data-slot="separator" and default orientation=horizontal', () => {
    render(<Separator />);
    const sep = document.querySelector('[data-slot="separator"]');
    expect(sep).not.toBeNull();
    expect(sep?.getAttribute('data-orientation')).toBe('horizontal');
  });

  it('supports orientation="vertical"', () => {
    render(<Separator orientation="vertical" />);
    const sep = document.querySelector('[data-slot="separator"]');
    expect(sep?.getAttribute('data-orientation')).toBe('vertical');
  });

  it('inline style references --fossil-colors-border token', () => {
    render(<Separator />);
    const sep = document.querySelector('[data-slot="separator"]') as HTMLElement;
    expect(sep.getAttribute('style') || '').toMatch(/var\(--fossil-colors-border\)/);
  });

  it('exposes role="separator" when decorative=false (semantic separator)', () => {
    render(<Separator decorative={false} />);
    // Radix marks decorative={false} separators with role="separator".
    expect(screen.getByRole('separator')).toBeTruthy();
  });
});
