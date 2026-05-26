/**
 * KanzoThemeProvider — Provider semantics + cascade observability.
 *
 * Three load-bearing assertions:
 *  1. Default render (no theme prop) applies the kanzoTheme cssVars on the
 *     wrapping div — distinguishing token: --fossil-fonts-sizeBase = 13px.
 *  2. Custom theme prop overrides the default — a spread + slot-override
 *     reaches the wrapping div's inline style.
 *  3. useKanzoTheme() returns the active context value when rendered under
 *     the Provider; throws outside.
 *
 * The cascade observability test (Reader pattern) mirrors plan 10-06's
 * useTheme.test.tsx pattern, adapted for the brand-owned ownership model.
 */

import { describe, it, expect } from 'vitest';
import { render, renderHook } from '@testing-library/react';
import * as React from 'react';
import {
  KanzoThemeProvider,
  useKanzoTheme,
} from '../src/KanzoThemeProvider.js';
import { kanzoTheme } from '../src/tokens.js';

describe('KanzoThemeProvider — default brand cascade', () => {
  it('renders a wrapping div carrying the kanzoTheme cssVars via inline style', () => {
    const { getByTestId } = render(
      <KanzoThemeProvider data-testid="provider-root">
        <span>child</span>
      </KanzoThemeProvider>,
    );
    const root = getByTestId('provider-root');
    expect(root.style.getPropertyValue('--fossil-fonts-sizeBase')).toBe(
      '13px',
    );
    expect(root.style.getPropertyValue('--fossil-fonts-sizeSmall')).toBe(
      '11px',
    );
    expect(root.style.getPropertyValue('--fossil-colors-background')).toBe(
      '#ffffff',
    );
    expect(root.style.getPropertyValue('--fossil-radii-md')).toBe('6px');
    expect(root.style.getPropertyValue('--fossil-focus-ring')).toMatch(/rgba/);
  });

  it('carries the data-kanzo-theme attribute on the wrapper (selector hook)', () => {
    const { container } = render(
      <KanzoThemeProvider>
        <span>child</span>
      </KanzoThemeProvider>,
    );
    expect(container.querySelector('[data-kanzo-theme]')).toBeTruthy();
  });
});

describe('KanzoThemeProvider — custom theme overrides', () => {
  it('a custom theme spread + slot-override reaches the wrapping style', () => {
    const customTheme = {
      ...kanzoTheme,
      colors: {
        ...kanzoTheme.colors,
        background: '#000000',
        accent: '#abcdef',
      },
    };
    const { getByTestId } = render(
      <KanzoThemeProvider theme={customTheme} data-testid="provider-root">
        <span>child</span>
      </KanzoThemeProvider>,
    );
    const root = getByTestId('provider-root');
    expect(root.style.getPropertyValue('--fossil-colors-background')).toBe(
      '#000000',
    );
    expect(root.style.getPropertyValue('--fossil-colors-accent')).toBe(
      '#abcdef',
    );
    // Un-overridden tokens fall through from the spread base.
    expect(root.style.getPropertyValue('--fossil-colors-foreground')).toBe(
      kanzoTheme.colors.foreground,
    );
  });

  it('user-supplied style prop merges AFTER cssVars (explicit wins)', () => {
    const { getByTestId } = render(
      <KanzoThemeProvider
        data-testid="provider-root"
        style={{ padding: '1rem' }}
      >
        <span>child</span>
      </KanzoThemeProvider>,
    );
    const root = getByTestId('provider-root');
    // cssVars still applied:
    expect(root.style.getPropertyValue('--fossil-fonts-sizeBase')).toBe(
      '13px',
    );
    // User style merged:
    expect(root.style.padding).toBe('1rem');
  });
});

describe('useKanzoTheme — context access', () => {
  function Reader(): React.ReactElement {
    const { theme, cssVars } = useKanzoTheme();
    return (
      <div>
        <span data-testid="size-base">{theme.fonts.sizeBase}</span>
        <span data-testid="cssvars-count">
          {Object.keys(cssVars).length}
        </span>
      </div>
    );
  }

  it('returns the active theme + flattened cssVars when rendered under a Provider', () => {
    const { getByTestId } = render(
      <KanzoThemeProvider>
        <Reader />
      </KanzoThemeProvider>,
    );
    expect(getByTestId('size-base').textContent).toBe('13px');
    // cssVars count is non-zero — the mechanical flatten produced a record.
    expect(Number(getByTestId('cssvars-count').textContent)).toBeGreaterThan(
      10,
    );
  });

  it('throws when called outside a Provider', () => {
    // Render a component that calls the hook with NO surrounding Provider.
    // renderHook silently swallows the throw in some configurations — use
    // a try/catch around the renderHook invocation to capture it explicitly.
    expect(() => renderHook(() => useKanzoTheme())).toThrow(
      /KanzoThemeProvider/,
    );
  });
});
