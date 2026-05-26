/**
 * themeToCssVars — vendored mechanical-flatten helper.
 *
 * Per ADR-0034 (CSS variable naming convention via mechanical flatten): a
 * FossilTheme object's nested-string-leaves are flattened to a flat
 * `Record<string, string>` keyed by `--fossil-${dot-path-joined-by-dashes}`.
 * Leaf string at path `[a, b, c]` becomes the CSS custom property
 * `--fossil-a-b-c`.
 *
 * Why vendored here (not imported from @fossil-lang/playground):
 *  - Keeps @kanzo/theme's dep graph tight (peer: react, dep:
 *    @fossil-lang/types only). Importing from playground would couple a
 *    brand-owned package to the runtime surface of an OSS one.
 *  - Both copies are governed by ADR-0034 — if the flattener semantics
 *    ever changed (extremely unlikely; the rule is intentionally mechanical
 *    + lossless), both copies update in lockstep via the ADR.
 *  - The duplication is ~25 lines — acceptable in exchange for the cleaner
 *    dep graph.
 *
 * @see decisions/0034-css-variable-naming-mechanical-flatten.md
 * @see decisions/0035-visual-ownership-separation.md
 */

import type { CSSProperties } from 'react';
import type { FossilTheme } from '@fossil-lang/types';

/** Shared CSS custom property prefix for every Fossil theme variable. */
export const CSS_VAR_PREFIX = '--fossil-';

/**
 * Flatten a FossilTheme into a `{ '--fossil-...': value }` record. Mechanical
 * rule: every leaf STRING (or coerced number/boolean) becomes a CSS variable;
 * nested objects are walked recursively with their key appended (dash-joined).
 *
 * Example:
 *   `{ colors: { syntax: { keyword: '#7c3aed' } } }`
 *   →  `{ '--fossil-colors-syntax-keyword': '#7c3aed' }`
 *
 * The output is suitable for direct splat into a React `style` prop.
 */
export function themeToCssVars(theme: FossilTheme): Record<string, string> {
  const vars: Record<string, string> = {};
  const walk = (obj: Record<string, unknown>, path: string[]): void => {
    for (const [k, v] of Object.entries(obj)) {
      const nextPath = [...path, k];
      if (typeof v === 'string') {
        vars[CSS_VAR_PREFIX + nextPath.join('-')] = v;
      } else if (typeof v === 'number' || typeof v === 'boolean') {
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
 * Cast a CSS variable record to a React `style` prop value. React's
 * CSSProperties type doesn't statically accept arbitrary `--*` keys, but
 * the runtime does — this cast is the canonical React community workaround.
 */
export function cssVarsToStyle(
  vars: Record<string, string>,
): CSSProperties {
  return vars as unknown as CSSProperties;
}
