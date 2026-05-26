/**
 * DropdownMenu primitive — unit tests. Covers render, data-slot anatomy,
 * highlighted state, ARIA menu role inheritance from Radix, focus surface
 * sentinel, and singleton stylesheet co-existence.
 *
 * Per Phase 10 plan 10-04 Task 2.
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuLabel,
} from '../src/primitives/DropdownMenu.js';

function renderMenu(defaultOpen = false) {
  return render(
    <DropdownMenu defaultOpen={defaultOpen}>
      <DropdownMenuTrigger>Open menu</DropdownMenuTrigger>
      <DropdownMenuContent>
        <DropdownMenuLabel>Section</DropdownMenuLabel>
        <DropdownMenuItem>One</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem>Two</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>,
  );
}

describe('DropdownMenu', () => {
  it('renders closed by default — trigger present, content absent', () => {
    renderMenu(false);
    expect(
      document.querySelector('[data-slot="dropdown-menu-trigger"]'),
    ).not.toBeNull();
    expect(
      document.querySelector('[data-slot="dropdown-menu-content"]'),
    ).toBeNull();
  });

  it('opens via defaultOpen, exposing content with full data-slot anatomy', () => {
    renderMenu(true);
    expect(
      document.querySelector('[data-slot="dropdown-menu-content"]'),
    ).not.toBeNull();
    expect(
      document.querySelectorAll('[data-slot="dropdown-menu-item"]'),
    ).toHaveLength(2);
    expect(
      document.querySelector('[data-slot="dropdown-menu-separator"]'),
    ).not.toBeNull();
    expect(
      document.querySelector('[data-slot="dropdown-menu-label"]'),
    ).not.toBeNull();
  });

  it('content has ARIA role="menu" inherited from Radix', () => {
    renderMenu(true);
    expect(screen.getByRole('menu')).toBeTruthy();
    // Each Item inherits role="menuitem".
    expect(screen.getAllByRole('menuitem')).toHaveLength(2);
  });

  it('content carries data-fossil-ui-primitive (focus-visible sentinel) + z-index inline style', () => {
    renderMenu(true);
    const content = document.querySelector(
      '[data-slot="dropdown-menu-content"]',
    ) as HTMLElement;
    expect(content.hasAttribute('data-fossil-ui-primitive')).toBe(true);
    expect(content.getAttribute('style') || '').toMatch(/z-index/);
  });

  it('co-exists with the 10-03 singleton stylesheet (still exactly one #fossil-ui-primitive-styles)', () => {
    renderMenu(true);
    const styleTags = document.querySelectorAll(
      '#fossil-ui-primitive-styles',
    );
    expect(styleTags).toHaveLength(1);
  });
});
