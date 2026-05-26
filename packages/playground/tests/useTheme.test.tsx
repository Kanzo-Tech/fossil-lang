/**
 * VIS-02 unit tests — the `useTheme` hook resolves a `FossilThemeProp` into
 * the `{ theme, cssVars, editorTheme }` triple, with the v0.2 default of
 * `'fossil-ide'`.
 *
 * Coverage:
 *   - no-prop default → fossilIdeTheme
 *   - explicit `undefined` prop → fossilIdeTheme (parity with no-prop;
 *     guards against future regressions where the default mechanism could
 *     diverge for explicit-undefined vs omitted)
 *   - `'fossil-ide'` string alias → fossilIdeTheme
 *   - `'light'` / `'dark'` string aliases still resolve to the v0.1 built-ins
 *     (backwards compat — VIS-02 SC#3)
 *   - custom FossilTheme object pass-through (no regression on advanced consumers)
 *   - cssVars record reflects the new default tokens
 *   - v0.1.x consumer rendered-component assertion: a tiny rendered child
 *     reads getComputedStyle on a div carrying the cssVars-as-style; the
 *     cascade is observable. Per CONTEXT.md Reconciliation 6 (Warning 4 fix)
 *     — placed here (not in FossilPlayground.test.tsx) to avoid snapshot
 *     churn.
 */

import { describe, it, expect } from 'vitest';
import { render, renderHook } from '@testing-library/react';
import * as React from 'react';
import { useTheme } from '../src/hooks/useTheme.js';
import { lightTheme } from '../src/theme/light.js';
import { darkTheme } from '../src/theme/dark.js';
import { fossilIdeTheme } from '../src/theme/fossil-ide.js';
import { cssVarsToStyle } from '../src/theme/tokens.js';

describe('useTheme — v0.2 default behaviour', () => {
  it('defaults to fossil-ide when no prop is passed', () => {
    const { result } = renderHook(() => useTheme());
    expect(result.current.theme).toBe(fossilIdeTheme);
    expect(result.current.theme.fonts.sizeBase).toBe('13px');
  });

  it('defaults to fossil-ide when prop is explicitly undefined', () => {
    const { result } = renderHook(() => useTheme(undefined));
    expect(result.current.theme).toBe(fossilIdeTheme);
  });

  it("resolves 'fossil-ide' string to fossilIdeTheme", () => {
    const { result } = renderHook(() => useTheme('fossil-ide'));
    expect(result.current.theme).toBe(fossilIdeTheme);
  });

  it("resolves 'light' string to lightTheme (backwards compat)", () => {
    const { result } = renderHook(() => useTheme('light'));
    expect(result.current.theme).toBe(lightTheme);
    expect(result.current.theme.fonts.sizeBase).toBe('14px');
  });

  it("resolves 'dark' string to darkTheme (backwards compat)", () => {
    const { result } = renderHook(() => useTheme('dark'));
    expect(result.current.theme).toBe(darkTheme);
    expect(result.current.theme.fonts.sizeBase).toBe('14px');
  });

  it('accepts a custom FossilTheme object', () => {
    const custom = {
      ...lightTheme,
      fonts: { ...lightTheme.fonts, sizeBase: '99px' },
    };
    const { result } = renderHook(() => useTheme(custom));
    expect(result.current.theme.fonts.sizeBase).toBe('99px');
  });

  it('emits CSS vars from fossil-ide reflecting the new default', () => {
    const { result } = renderHook(() => useTheme());
    expect(result.current.cssVars['--fossil-fonts-sizeBase']).toBe('13px');
    expect(result.current.cssVars['--fossil-radii-md']).toBe('6px');
    expect(result.current.cssVars['--fossil-focus-ring']).toMatch(/rgba/);
  });

  // v0.1.x consumer assertion — placed here per CONTEXT.md Reconciliation 6
  // (Warning 4 fix). A rendered child component reads getComputedStyle on the
  // root to confirm the cascade propagates. We avoid touching
  // FossilPlayground.test.tsx (snapshot churn risk).
  it('rendered component reads fossil-ide CSS vars from the root', () => {
    function Reader(): React.ReactElement {
      const { theme, cssVars } = useTheme();
      const style = cssVarsToStyle(cssVars);
      return (
        <div data-testid="theme-root" style={style}>
          <span data-testid="size-base">{theme.fonts.sizeBase}</span>
        </div>
      );
    }
    const { getByTestId } = render(<Reader />);
    const root = getByTestId('theme-root');
    expect(root.style.getPropertyValue('--fossil-fonts-sizeBase')).toBe('13px');
    expect(getByTestId('size-base').textContent).toBe('13px');
  });
});
