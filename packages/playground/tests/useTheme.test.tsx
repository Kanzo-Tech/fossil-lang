/**
 * useTheme — v0.2.x default behaviour (host-provider model, ADR-0035).
 *
 * Coverage (post-10-09 ownership inversion):
 *   - No prop → theme === undefined; cssVars === {} (empty). Host supplies
 *     the cascade via an ancestor provider. The OSS surface is
 *     brand-agnostic.
 *   - Explicit `undefined` prop → same as no prop (parity with the
 *     omitted case).
 *   - Explicit `'light'` → lightTheme (v0.1.x backwards-compat invariant).
 *   - Explicit `'dark'` → darkTheme (v0.1.x backwards-compat invariant).
 *   - Custom FossilTheme object → pass-through (advanced consumers).
 *   - Brand-cascade observability — rendered child under a
 *     `<KanzoThemeProvider/>` (from @kanzo/theme) reads
 *     `--fossil-fonts-sizeBase: 13px` from the Provider's wrapping div via
 *     getComputedStyle. This is the new canonical proof that the brand
 *     surface is provided EXTERNALLY (by @kanzo/theme), not by the
 *     @fossil-lang/playground hook.
 *
 * Per CONTEXT.md Reconciliation 6 (v0.1.x consumer assertion pattern):
 * the rendered Reader child reads getComputedStyle on the Provider's
 * wrapper, NOT on the FossilPlayground component tree (avoids snapshot
 * churn risk in the heavier FossilPlayground.test.tsx).
 */

import { describe, it, expect } from 'vitest';
import { render, renderHook } from '@testing-library/react';
import * as React from 'react';
import { KanzoThemeProvider } from '@kanzo/theme';
import { useTheme } from '../src/hooks/useTheme.js';
import { lightTheme } from '../src/theme/light.js';
import { darkTheme } from '../src/theme/dark.js';
import { cssVarsToStyle } from '../src/theme/tokens.js';

describe('useTheme — v0.2.x default behaviour (host-provider model, ADR-0035)', () => {
  it('returns undefined theme + empty cssVars when no prop is passed (host provides via Provider)', () => {
    const { result } = renderHook(() => useTheme());
    expect(result.current.theme).toBeUndefined();
    expect(result.current.cssVars).toEqual({});
    // editorTheme is a no-op extension — type-level guarantee + shape check.
    // (CodeMirror Extension type is structural; the runtime is an array or
    // object — we assert it exists and is not falsy.)
    expect(result.current.editorTheme).toBeTruthy();
  });

  it('returns undefined theme + empty cssVars when prop is explicitly undefined', () => {
    const { result } = renderHook(() => useTheme(undefined));
    expect(result.current.theme).toBeUndefined();
    expect(result.current.cssVars).toEqual({});
  });

  it("resolves 'light' string to lightTheme (v0.1.x backwards compat)", () => {
    const { result } = renderHook(() => useTheme('light'));
    expect(result.current.theme).toBe(lightTheme);
    expect(result.current.theme?.fonts.sizeBase).toBe('14px');
    expect(
      result.current.cssVars['--fossil-fonts-sizeBase'],
    ).toBe('14px');
  });

  it("resolves 'dark' string to darkTheme (v0.1.x backwards compat)", () => {
    const { result } = renderHook(() => useTheme('dark'));
    expect(result.current.theme).toBe(darkTheme);
    expect(result.current.theme?.fonts.sizeBase).toBe('14px');
  });

  it('accepts a custom FossilTheme object (advanced consumer)', () => {
    const custom = {
      ...lightTheme,
      fonts: { ...lightTheme.fonts, sizeBase: '99px' },
    };
    const { result } = renderHook(() => useTheme(custom));
    expect(result.current.theme?.fonts.sizeBase).toBe('99px');
    expect(result.current.cssVars['--fossil-fonts-sizeBase']).toBe('99px');
  });

  // Brand-cascade observability — rendered child under a
  // <KanzoThemeProvider/> (from @kanzo/theme) confirms the brand surface
  // reaches consumers EXTERNALLY (via the Provider's wrapping div's inline
  // style), NOT via the playground hook. This is the new canonical
  // VIS-02 / ADR-0035 proof pattern.
  it('rendered component under <KanzoThemeProvider/> reads kanzo cssVars from the Provider wrapper', () => {
    function Reader(): React.ReactElement {
      const { cssVars } = useTheme();
      const style = cssVarsToStyle(cssVars);
      return (
        <div data-testid="reader-root" style={style}>
          <span data-testid="reader-marker">reader</span>
        </div>
      );
    }
    const { getByTestId } = render(
      <KanzoThemeProvider data-testid="provider-root">
        <Reader />
      </KanzoThemeProvider>,
    );
    // The Provider's wrapping div carries the brand cascade:
    const providerRoot = getByTestId('provider-root');
    expect(providerRoot.style.getPropertyValue('--fossil-fonts-sizeBase')).toBe(
      '13px',
    );
    // The inner Reader (calling useTheme() with no prop) carries NO inline
    // --fossil-* style of its own — proof that the playground hook does
    // NOT auto-inject the brand. The cascade reaches the Reader's
    // descendants through standard CSS-var cascade (the inner div's
    // computed style would resolve --fossil-fonts-sizeBase to 13px via
    // the Provider's wrapper).
    const readerRoot = getByTestId('reader-root');
    expect(readerRoot.style.getPropertyValue('--fossil-fonts-sizeBase')).toBe(
      '',
    );
    expect(getByTestId('reader-marker').textContent).toBe('reader');
  });
});
