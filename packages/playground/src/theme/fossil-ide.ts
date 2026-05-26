/**
 * fossil-ide — the DEFAULT v0.2+ theme. Extends `lightTheme` with IDE-specific
 * specialisations: slightly tighter base typography (13px vs 14px) to match
 * the visual density of professional IDEs (VSCode, Cursor, Keasy). Colors,
 * spacing, radii, motion are inherited from `lightTheme` (which already
 * contains the IDE-grade token namespaces post Phase 10 plan 10-02).
 *
 * Why a SEPARATE theme value (vs just mutating `lightTheme`):
 *   - `lightTheme` is the v0.1 default — consumers passing `theme='light'`
 *     must see IDENTICAL behaviour to v0.1.x (no surprise upgrades). The
 *     ONLY surface change is the default when `theme` prop is omitted.
 *   - Hosts can opt INTO the IDE look explicitly via `theme='fossil-ide'`
 *     OR opt OUT via `theme='light'`.
 *
 * NARROW SCOPE in v0.2.0 (per CONTEXT.md Reconciliation 5): the only
 * intentional divergence from `lightTheme` is `fonts.sizeBase` (13px) and
 * `fonts.sizeSmall` (11px). Every other token (colors, radii, spacing,
 * motion, focus, size.control) is inherited unchanged. The "IDE feel" is
 * delivered primarily by the new primitives shipped in @fossil-lang/ui
 * (plans 10-03/04/05) which read IDE-grade tokens from the shared theme;
 * this file's contribution is the typography tightening.
 *
 * The "IDE feel" delivered by v0.2.0 fossil-ide comes from THREE composing
 * sources (this theme provides only #2; #1 + #3 come from Plans 10-03/04/05
 * + 10-07):
 *   1. Radix primitives with Keasy-equivalent density + variants
 *   2. Slightly smaller base typography (this file)
 *   3. Phase 14 layout refactor (tabs IDE-style)
 *
 * A fossil-ide-dark variant is deferred to Phase 14 — for v0.2.0 consumers
 * passing `theme='dark'` continue to get `darkTheme` which already has the
 * IDE-grade tokens from 10-02.
 *
 * Phase 10 VIS-02.
 */
import type { FossilTheme } from '@fossil-lang/types';
import { lightTheme } from './light.js';

export const fossilIdeTheme: FossilTheme = {
  ...lightTheme,
  fonts: {
    ...lightTheme.fonts,
    /** 13px — IDE-grade base (was 14px in lightTheme). Closer to VSCode default editor font size. */
    sizeBase: '13px',
    /** 11px — IDE-grade small (was 12px in lightTheme). */
    sizeSmall: '11px',
  },
};
