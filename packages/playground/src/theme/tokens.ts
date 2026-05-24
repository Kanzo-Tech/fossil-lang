/**
 * Theme tokens — the canonical mapping from the structured `FossilTheme`
 * (defined in `@fossil-lang/types`) onto a flat CSS custom property record.
 *
 * Why this layer exists:
 *   - The `FossilTheme` shape is hierarchical (colors.syntax.keyword, fonts.mono,
 *     space.md, ...). CSS custom properties are flat. This module is the
 *     canonical flattener — `colors.syntax.keyword` → `--fossil-colors-syntax-keyword`.
 *   - The flat record is applied to the playground root via React's `style`
 *     prop (browsers treat unknown `--*` keys as custom properties). Once set
 *     there, every descendant — including the CodeMirror editor host — can
 *     read the same variable, AND any ancestor of the playground can override
 *     a single token via the same CSS-variable cascade WITHOUT touching the
 *     `theme` prop. This is the Monaco-style override API CONTEXT.md calls
 *     for + ADR-0028's reusable-component contract.
 *   - Consumers of `themeToCssVars` are: `useTheme` (the hook that applies the
 *     variables) + future test code that wants to assert the resolved values.
 *
 * The flattening rule is deliberately mechanical: every leaf string in the
 * theme object becomes `--fossil-<dot-path-joined-by-dashes>`. No special-
 * casing — adding new tokens to `FossilTheme` (e.g. `colors.surface.elevated`)
 * automatically yields `--fossil-colors-surface-elevated` with zero code
 * changes here.
 */

import type { CSSProperties } from 'react';
import type { FossilTheme } from '@fossil-lang/types';

/** The CSS custom property prefix every Fossil theme variable shares. */
export const CSS_VAR_PREFIX = '--fossil-';

/**
 * Flatten a `FossilTheme` into a `{ '--fossil-...': value }` record.
 *
 * Mechanical rule: every leaf STRING becomes a CSS variable; nested objects
 * are walked recursively with their key appended (dash-joined). Non-string
 * leaves (numbers, booleans) are not part of the `FossilTheme` shape today
 * but would be coerced to strings on the way out — kept defensive in case
 * future tokens add e.g. opacity numbers.
 *
 * Example: `{ colors: { syntax: { keyword: '#7c3aed' } } }`
 *       → `{ '--fossil-colors-syntax-keyword': '#7c3aed' }`
 *
 * The output is suitable for direct splat into a React `style` prop (browsers
 * preserve `--*` properties verbatim). See `cssVarsToStyle` for the typed cast.
 */
export function themeToCssVars(theme: FossilTheme): Record<string, string> {
  const vars: Record<string, string> = {};
  const walk = (obj: Record<string, unknown>, path: string[]): void => {
    for (const [k, v] of Object.entries(obj)) {
      const nextPath = [...path, k];
      if (typeof v === 'string') {
        vars[CSS_VAR_PREFIX + nextPath.join('-')] = v;
      } else if (typeof v === 'number' || typeof v === 'boolean') {
        // Defensive: not part of the shape today, but cheap to support.
        vars[CSS_VAR_PREFIX + nextPath.join('-')] = String(v);
      } else if (v && typeof v === 'object') {
        walk(v as Record<string, unknown>, nextPath);
      }
    }
  };
  walk(theme as unknown as Record<string, unknown>, []);
  return vars;
}

/**
 * Cast a CSS variable record to a React `style` prop value.
 *
 * React's `CSSProperties` type doesn't statically accept arbitrary `--*` keys,
 * but the runtime DOES — this cast is the canonical workaround the React
 * community uses (and the React team has acknowledged as expected; see
 * https://github.com/facebook/react/pull/9302). Centralising it here keeps
 * the unsafe-looking cast in ONE auditable location.
 */
export function cssVarsToStyle(
  vars: Record<string, string>,
): CSSProperties {
  return vars as unknown as CSSProperties;
}
