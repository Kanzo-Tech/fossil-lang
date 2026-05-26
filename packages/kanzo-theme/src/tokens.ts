/**
 * kanzoTheme — the canonical brand-owned FossilTheme value for kanzo-branded
 * hosts (apps/landing, future Keasy migration consumers).
 *
 * Per ADR-0035 (visual ownership separation): brand visuals are owned by the
 * @kanzo/* family, NOT by @fossil-lang/*. This file is the v0.2 replacement
 * for `packages/playground/src/theme/fossil-ide.ts` (which lived inside
 * @fossil-lang/playground after plan 10-06 — an incorrect coupling that
 * 10-09 corrects).
 *
 * Token VALUES are sourced from the @fossil-lang/playground lightTheme
 * baseline (packages/playground/src/theme/light.ts) with the kanzo IDE-grade
 * typography divergence (fonts.sizeBase=13px + fonts.sizeSmall=11px). The
 * values are INLINED here — NOT imported from @fossil-lang/playground — so:
 *
 *  1. @kanzo/theme has a tight dep graph (peer: react, dep: @fossil-lang/types
 *     for the FossilTheme type contract). No runtime dep on the OSS surface.
 *  2. Future divergence is expected — this is a brand-owned palette. As the
 *     kanzo visual identity evolves (and it will, independent of OSS Fossil
 *     development), this file changes WITHOUT touching @fossil-lang/playground.
 *  3. Cycle prevention — if @fossil-lang/playground ever wanted to import
 *     @kanzo/theme for testing or examples purposes (it doesn't today), that
 *     dep direction would only be safe because @kanzo/theme does not pull
 *     back into the playground.
 *
 * The shape conforms to the FossilTheme type contract from @fossil-lang/types
 * — every token slot is filled per ADR-0034 mechanical-flatten conventions.
 *
 * @see decisions/0035-visual-ownership-separation.md
 * @see decisions/0034-css-variable-naming-mechanical-flatten.md
 * @see decisions/0033-fossil-lang-ui-package.md (Amendment 2026-05-26)
 */

import type { FossilTheme } from '@fossil-lang/types';

/**
 * The default kanzo brand theme. Hosts wrap their tree in
 * `<KanzoThemeProvider/>` (which defaults to this value) to apply the
 * `--fossil-*` CSS-var cascade. Custom variants can spread this object and
 * override individual slots — `{ ...kanzoTheme, colors: { ...kanzoTheme.colors, accent: '#ff00ff' } }`.
 */
export const kanzoTheme: FossilTheme = {
  colors: {
    /** App chrome background — pure white maximises contrast headroom. */
    background: '#ffffff',
    /** Primary body text. slate-900 on #ffffff ≈ 15.4:1 (AAA). */
    foreground: '#0f172a',
    /** De-emphasised text (gutters, captions). slate-500 ≈ 4.6:1 — just
     *  above AA's 4.5:1 floor. */
    muted: '#64748b',
    /** Borders + dividers — decorative, no contrast requirement. */
    border: '#e2e8f0',
    /** Focus rings + selection highlight + primary action. blue-500. */
    accent: '#3b82f6',
    /** Error state. red-600 on #ffffff ≈ 5.9:1 (AA for body text). */
    error: '#dc2626',
    /** Warning state. amber-700 ≈ 5.0:1 (AA). */
    warning: '#b45309',
    /** Informational. sky-700 ≈ 5.9:1 (AA). */
    info: '#0369a1',
    /** Focus-ring colour anchor (Phase 10 VIS-03 + ADR-0034). Default =
     *  accent. Hosts override this slot independently of accent if they
     *  want a distinct ring colour. */
    ring: '#3b82f6',
    syntax: {
      /** violet-600 ≈ 5.7:1 (AA). */
      keyword: '#7c3aed',
      /** green-600 ≈ 3.3:1 (decorative). */
      string: '#16a34a',
      /** orange-600 ≈ 3.6:1 (decorative). */
      number: '#ea580c',
      /** slate-400 ≈ 2.6:1 — annotative comments, italicised in the
       *  CodeMirror highlight style. */
      comment: '#94a3b8',
      /** Identifiers fall back to foreground (best contrast). */
      identifier: '#0f172a',
      /** slate-600 ≈ 7.6:1 (AAA). */
      operator: '#475569',
      /** slate-500 ≈ 4.8:1 (AA). */
      punctuation: '#64748b',
      /** cyan-600 ≈ 3.7:1 (decorative). */
      prefixedName: '#0891b2',
      /** Same as keyword — IRIs visually distinguished from identifiers
       *  via italic in the CodeMirror highlight style. */
      iri: '#7c3aed',
    },
  },
  fonts: {
    sans: "ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif",
    mono: "ui-monospace, 'JetBrains Mono', 'Fira Code', Menlo, monospace",
    /** 13px — IDE-grade base (matches VSCode default editor font size).
     *  +1px tighter than the @fossil-lang/playground lightTheme baseline
     *  (14px) — this is the kanzo-brand typography signature. */
    sizeBase: '13px',
    /** 11px — IDE-grade small. +1px tighter than lightTheme (12px). */
    sizeSmall: '11px',
  },
  space: {
    xs: '4px',
    sm: '8px',
    md: '16px',
    lg: '24px',
    xl: '32px',
  },
  radius: {
    sm: '4px',
    md: '8px',
  },
  // ─── IDE-grade token namespaces (ADR-0034) ───
  radii: {
    sm: '4px',
    md: '6px',
    lg: '8px',
    xl: '12px',
    full: '9999px',
  },
  spacing: {
    '0': '0',
    '1': '4px',
    '2': '8px',
    '3': '12px',
    '4': '16px',
    '6': '24px',
    '8': '32px',
  },
  motion: {
    duration: {
      fast: '150ms',
      base: '200ms',
    },
    easing: 'cubic-bezier(0.4, 0, 0.2, 1)',
  },
  focus: {
    /** Pre-resolved rgba — derived from colors.accent (#3b82f6 →
     *  R=59 G=130 B=246) at 50% opacity. Per ADR-0034 rule 4 (Safari <16.2
     *  compat — modern CSS colour-blending functions are unsupported there). */
    ring: '0 0 0 3px rgba(59, 130, 246, 0.5)',
  },
  size: {
    control: {
      base: '32px',
      sm: '24px',
      lg: '40px',
    },
  },
};
