/**
 * VIS-02 unit tests — `fossilIdeTheme` value composition.
 *
 * The narrow scope in v0.2.0 (per CONTEXT.md Reconciliation 5) is the
 * load-bearing invariant for this suite:
 *   - colors, radii, spacing, motion, focus, size are INHERITED from
 *     lightTheme (verbatim — `.toEqual` covers structural equality);
 *   - fonts.sizeBase and fonts.sizeSmall are OVERRIDDEN to the IDE-grade
 *     values (13px / 11px);
 *   - fonts.mono and fonts.sans flow through unchanged from lightTheme.
 *
 * These tests guard against accidental widening of the divergence — if a
 * future contributor reaches for fossil-ide to ship a colour palette change,
 * the inheritance assertions will fail and force the discussion: either
 * widen the scope deliberately (and update these tests + CONTEXT.md) or
 * keep the narrow contract.
 */

import { describe, it, expect } from 'vitest';
import { fossilIdeTheme } from '../src/theme/fossil-ide.js';
import { lightTheme } from '../src/theme/light.js';

describe('fossilIdeTheme — composition over lightTheme', () => {
  it('inherits all color tokens from lightTheme', () => {
    expect(fossilIdeTheme.colors).toEqual(lightTheme.colors);
  });

  it('inherits radii, spacing, motion, focus, size from lightTheme', () => {
    expect(fossilIdeTheme.radii).toEqual(lightTheme.radii);
    expect(fossilIdeTheme.spacing).toEqual(lightTheme.spacing);
    expect(fossilIdeTheme.motion).toEqual(lightTheme.motion);
    expect(fossilIdeTheme.focus).toEqual(lightTheme.focus);
    expect(fossilIdeTheme.size).toEqual(lightTheme.size);
  });

  it('overrides font sizes for IDE-grade typography', () => {
    expect(fossilIdeTheme.fonts.sizeBase).toBe('13px');
    expect(fossilIdeTheme.fonts.sizeSmall).toBe('11px');
    expect(fossilIdeTheme.fonts.mono).toBe(lightTheme.fonts.mono);
    expect(fossilIdeTheme.fonts.sans).toBe(lightTheme.fonts.sans);
  });
});
