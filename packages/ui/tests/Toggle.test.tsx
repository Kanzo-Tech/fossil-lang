/**
 * Toggle + ToggleGroup primitives — unit tests. Covers render, data-slot
 * anatomy, variant prop, controlled pressed state, ToggleGroup context
 * propagation, and ARIA role inheritance from Radix.
 *
 * Per Phase 10 plan 10-05 Task 2.
 *
 * Note: We use the controlled `pressed` prop on <Toggle> (and `value` on
 * <ToggleGroup>) to assert the `data-state="on"` rendering without depending
 * on happy-dom's event timing — same pattern as the Tooltip + DropdownMenu
 * tests from plan 10-04.
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import {
  Toggle,
  ToggleGroup,
  ToggleGroupItem,
} from '../src/primitives/Toggle.js';

describe('Toggle', () => {
  it('renders with data-slot="toggle" and default data-variant="default"', () => {
    render(<Toggle aria-label="Bold">B</Toggle>);
    const toggle = document.querySelector('[data-slot="toggle"]') as HTMLElement;
    expect(toggle).not.toBeNull();
    expect(toggle.getAttribute('data-variant')).toBe('default');
    // Radix sets data-state="off" by default for an uncontrolled, unpressed
    // toggle.
    expect(toggle.getAttribute('data-state')).toBe('off');
  });

  it('forwards variant="outline" to the data-variant attribute', () => {
    render(
      <Toggle variant="outline" aria-label="Italic">
        I
      </Toggle>,
    );
    const toggle = document.querySelector('[data-slot="toggle"]') as HTMLElement;
    expect(toggle.getAttribute('data-variant')).toBe('outline');
  });

  it('controlled pressed=true renders data-state="on"', () => {
    render(
      <Toggle pressed aria-label="Bold">
        B
      </Toggle>,
    );
    const toggle = document.querySelector('[data-slot="toggle"]') as HTMLElement;
    expect(toggle.getAttribute('data-state')).toBe('on');
  });

  it('exposes ARIA role="button" with aria-pressed inherited from Radix', () => {
    render(
      <Toggle pressed aria-label="Bold">
        B
      </Toggle>,
    );
    // Radix's Toggle renders a <button> with aria-pressed.
    const btn = screen.getByRole('button', { name: 'Bold' });
    expect(btn).toBeTruthy();
    expect(btn.getAttribute('aria-pressed')).toBe('true');
  });

  it('co-exists with the singleton stylesheet (exactly one #fossil-ui-primitive-styles)', () => {
    render(<Toggle aria-label="Bold">B</Toggle>);
    const styleTags = document.querySelectorAll('#fossil-ui-primitive-styles');
    expect(styleTags).toHaveLength(1);
  });
});

describe('ToggleGroup', () => {
  it('renders group root with data-slot="toggle-group" + data-variant', () => {
    render(
      <ToggleGroup type="multiple" variant="outline" aria-label="Formatting">
        <ToggleGroupItem value="bold" aria-label="Bold">
          B
        </ToggleGroupItem>
        <ToggleGroupItem value="italic" aria-label="Italic">
          I
        </ToggleGroupItem>
      </ToggleGroup>,
    );
    const group = document.querySelector(
      '[data-slot="toggle-group"]',
    ) as HTMLElement;
    expect(group).not.toBeNull();
    expect(group.getAttribute('data-variant')).toBe('outline');
  });

  it('propagates variant to children via context — items inherit data-variant="outline"', () => {
    render(
      <ToggleGroup type="multiple" variant="outline" aria-label="Formatting">
        <ToggleGroupItem value="bold" aria-label="Bold">
          B
        </ToggleGroupItem>
        <ToggleGroupItem value="italic" aria-label="Italic">
          I
        </ToggleGroupItem>
      </ToggleGroup>,
    );
    // ToggleGroupItem renders data-slot="toggle" (intentional — single source
    // of CSS truth with standalone Toggle).
    const items = document.querySelectorAll(
      '[data-slot="toggle-group"] [data-slot="toggle"]',
    );
    expect(items).toHaveLength(2);
    items.forEach((item) => {
      expect(item.getAttribute('data-variant')).toBe('outline');
    });
  });

  it('controlled value renders selected item with data-state="on"', () => {
    render(
      <ToggleGroup
        type="single"
        value="bold"
        aria-label="Formatting"
      >
        <ToggleGroupItem value="bold" aria-label="Bold">
          B
        </ToggleGroupItem>
        <ToggleGroupItem value="italic" aria-label="Italic">
          I
        </ToggleGroupItem>
      </ToggleGroup>,
    );
    const items = document.querySelectorAll('[data-slot="toggle"]');
    const states = Array.from(items).map((i) => i.getAttribute('data-state'));
    expect(states).toContain('on');
    expect(states).toContain('off');
  });

  it('ToggleGroupItem allows per-item variant override of the group default', () => {
    render(
      <ToggleGroup type="multiple" variant="default" aria-label="Mixed">
        <ToggleGroupItem value="a" variant="outline" aria-label="A">
          A
        </ToggleGroupItem>
        <ToggleGroupItem value="b" aria-label="B">
          B
        </ToggleGroupItem>
      </ToggleGroup>,
    );
    const items = document.querySelectorAll('[data-slot="toggle"]');
    expect(items[0]?.getAttribute('data-variant')).toBe('outline');
    expect(items[1]?.getAttribute('data-variant')).toBe('default');
  });
});
