/**
 * @kanzo/theme — public entry point.
 *
 * Brand-owned visual layer for kanzo-branded hosts. Per ADR-0035 (visual
 * ownership separation): @fossil-lang/* OSS packages are theme-less; this
 * package supplies the kanzo brand palette + typography via React Context
 * + the `--fossil-*` CSS-var cascade.
 *
 * Stable API:
 *  - `kanzoTheme`: canonical brand FossilTheme value
 *  - `KanzoThemeProvider`: React component wrapping a subtree with the
 *    brand cascade
 *  - `useKanzoTheme`: hook to read the active brand theme from within a
 *    Provider
 *  - `themeToCssVars` / `cssVarsToStyle` / `CSS_VAR_PREFIX`: the vendored
 *    mechanical-flatten helpers (per ADR-0034)
 *
 * @see decisions/0035-visual-ownership-separation.md
 * @see decisions/0034-css-variable-naming-mechanical-flatten.md
 */

export { kanzoTheme } from './tokens.js';
export {
  KanzoThemeProvider,
  useKanzoTheme,
} from './KanzoThemeProvider.js';
export type { KanzoThemeProviderProps } from './KanzoThemeProvider.js';
export {
  themeToCssVars,
  cssVarsToStyle,
  CSS_VAR_PREFIX,
} from './themeToCssVars.js';
