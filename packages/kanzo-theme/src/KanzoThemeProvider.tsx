/**
 * KanzoThemeProvider — wraps a React subtree in a `<div>` carrying the
 * `--fossil-*` CSS-var cascade for the kanzo brand surface.
 *
 * Per ADR-0035 (visual ownership separation): brand visuals are owned by the
 * @kanzo/* family. @fossil-lang/ui primitives are theme-less and consume
 * the `--fossil-*` CSS-var contract at runtime. The host wraps its tree in
 * this Provider to install the kanzo brand palette + typography for every
 * descendant — including `<FossilPlayground/>` and any standalone primitive.
 *
 * Usage (apps/landing pattern):
 *
 *   import { KanzoThemeProvider } from '@kanzo/theme';
 *   import { FossilPlayground } from '@fossil-lang/playground';
 *
 *   <KanzoThemeProvider>
 *     <FossilPlayground resolver={resolver} wasmUrl={wasmUrl} />
 *   </KanzoThemeProvider>
 *
 * Custom palette variant (advanced — spread kanzoTheme + override slots):
 *
 *   import { kanzoTheme, KanzoThemeProvider } from '@kanzo/theme';
 *
 *   const customTheme = {
 *     ...kanzoTheme,
 *     colors: { ...kanzoTheme.colors, accent: '#ff00ff' },
 *   };
 *   <KanzoThemeProvider theme={customTheme}>...</KanzoThemeProvider>
 *
 * The Provider also exposes a `useKanzoTheme()` hook for downstream consumers
 * that want to read the theme programmatically (e.g. a custom palette swatch).
 *
 * @see decisions/0035-visual-ownership-separation.md
 */

import {
  createContext,
  useContext,
  useMemo,
  type CSSProperties,
  type ReactNode,
} from 'react';
import type { FossilTheme } from '@fossil-lang/types';
import { kanzoTheme } from './tokens.js';
import { themeToCssVars, cssVarsToStyle } from './themeToCssVars.js';

interface KanzoThemeContextValue {
  theme: FossilTheme;
  cssVars: Record<string, string>;
}

const KanzoThemeContext = createContext<KanzoThemeContextValue | null>(null);

export interface KanzoThemeProviderProps {
  /**
   * Theme to apply. Defaults to the canonical `kanzoTheme`. Pass a custom
   * FossilTheme (spread kanzoTheme + override slots) for a brand variant.
   * Memoise the object via `useMemo` to avoid recomputing cssVars on every
   * render.
   */
  theme?: FossilTheme;
  /** Optional inline style overrides on the wrapping div. */
  style?: CSSProperties;
  /** Optional className on the wrapping div. */
  className?: string;
  /**
   * Optional `data-testid` attribute on the wrapping div. Useful in tests
   * that need to assert the cascade is observable at the provider boundary.
   */
  'data-testid'?: string;
  children?: ReactNode;
}

/**
 * Mount the kanzo brand theme — every `--fossil-*` CSS custom property
 * cascades to descendants through the wrapping div's inline style.
 */
export function KanzoThemeProvider({
  theme = kanzoTheme,
  style,
  className,
  'data-testid': testId,
  children,
}: KanzoThemeProviderProps): JSX.Element {
  const value = useMemo<KanzoThemeContextValue>(() => {
    return { theme, cssVars: themeToCssVars(theme) };
  }, [theme]);

  // Merge the user-provided style (if any) AFTER the cssVars so explicit
  // overrides win (React's style prop is shallow-merge; subsequent keys
  // overwrite). The wrapping div is non-semantic (`data-kanzo-theme`) and
  // does not introduce an extra layout box beyond the user's tree — display
  // defaults to block which mirrors the typical app-root container.
  const mergedStyle: CSSProperties = useMemo(
    () => ({
      ...cssVarsToStyle(value.cssVars),
      ...(style ?? {}),
    }),
    [value.cssVars, style],
  );

  return (
    <KanzoThemeContext.Provider value={value}>
      <div
        data-kanzo-theme=""
        data-testid={testId}
        className={className}
        style={mergedStyle}
      >
        {children}
      </div>
    </KanzoThemeContext.Provider>
  );
}

/**
 * Read the current kanzo theme + flattened CSS-var record from within a
 * `<KanzoThemeProvider/>`. Throws if called outside a Provider — the brand
 * surface is opt-in; consumers that haven't installed it shouldn't reach
 * for the hook.
 */
export function useKanzoTheme(): KanzoThemeContextValue {
  const ctx = useContext(KanzoThemeContext);
  if (!ctx) {
    throw new Error(
      'useKanzoTheme: must be called inside a <KanzoThemeProvider/>. See ADR-0035 for the brand-ownership story.',
    );
  }
  return ctx;
}
