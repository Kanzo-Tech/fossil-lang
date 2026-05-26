/**
 * kanzoTheme — brand value matrix lock.
 *
 * These tests pin down the kanzo brand's distinguishing tokens so future
 * accidental edits surface immediately. The brand-signature divergences from
 * the @fossil-lang/playground lightTheme baseline are:
 *
 *  - fonts.sizeBase: 13px (vs lightTheme 14px) — IDE-grade typography
 *  - fonts.sizeSmall: 11px (vs lightTheme 12px) — IDE-grade typography
 *
 * The rest of the palette inherits the lightTheme baseline (inlined into
 * kanzoTheme per ADR-0035 — brand-owned values are duplicated, not imported,
 * to keep the dep graph tight). A subset of structural assertions covers
 * the IDE-grade namespaces (radii, spacing, motion, focus, size) so the
 * mechanical-flatten contract per ADR-0034 stays satisfied.
 */

import { describe, it, expect } from 'vitest';
import { kanzoTheme } from '../src/tokens.js';

describe('kanzoTheme — brand-signature typography', () => {
  it('fonts.sizeBase is 13px (IDE-grade tightening; +1px vs lightTheme)', () => {
    expect(kanzoTheme.fonts.sizeBase).toBe('13px');
  });

  it('fonts.sizeSmall is 11px (IDE-grade tightening; +1px vs lightTheme)', () => {
    expect(kanzoTheme.fonts.sizeSmall).toBe('11px');
  });

  it('fonts.mono is the canonical Fossil mono stack', () => {
    expect(kanzoTheme.fonts.mono).toMatch(/JetBrains Mono/);
  });
});

describe('kanzoTheme — IDE-grade namespaces (ADR-0034)', () => {
  it('radii namespace populated (sm/md/lg/xl/full)', () => {
    expect(kanzoTheme.radii.sm).toBe('4px');
    expect(kanzoTheme.radii.md).toBe('6px');
    expect(kanzoTheme.radii.lg).toBe('8px');
    expect(kanzoTheme.radii.xl).toBe('12px');
    expect(kanzoTheme.radii.full).toBe('9999px');
  });

  it('spacing scale aligns with Tailwind (numeric-string keys)', () => {
    expect(kanzoTheme.spacing['0']).toBe('0');
    expect(kanzoTheme.spacing['1']).toBe('4px');
    expect(kanzoTheme.spacing['2']).toBe('8px');
    expect(kanzoTheme.spacing['4']).toBe('16px');
    expect(kanzoTheme.spacing['8']).toBe('32px');
  });

  it('motion tokens populated (sub-object nesting per ADR-0034 rule 1)', () => {
    expect(kanzoTheme.motion.duration.fast).toBe('150ms');
    expect(kanzoTheme.motion.duration.base).toBe('200ms');
    expect(kanzoTheme.motion.easing).toMatch(/cubic-bezier/);
  });

  it('focus.ring is pre-resolved rgba (no color-mix per ADR-0034 rule 4)', () => {
    expect(kanzoTheme.focus.ring).toBe(
      '0 0 0 3px rgba(59, 130, 246, 0.5)',
    );
    expect(kanzoTheme.focus.ring).not.toMatch(/color-mix/);
  });

  it('size.control namespace populated (base/sm/lg per ADR-0034 rule 7)', () => {
    expect(kanzoTheme.size.control.base).toBe('32px');
    expect(kanzoTheme.size.control.sm).toBe('24px');
    expect(kanzoTheme.size.control.lg).toBe('40px');
  });
});

describe('kanzoTheme — colors slot completeness', () => {
  it('exposes the focus-ring colour anchor (Phase 10 VIS-03)', () => {
    expect(kanzoTheme.colors.ring).toBe('#3b82f6');
    // Default == accent for the canonical brand value; hosts override to
    // decouple ring colour from accent if needed.
    expect(kanzoTheme.colors.ring).toBe(kanzoTheme.colors.accent);
  });

  it('background is white (kanzo brand baseline)', () => {
    expect(kanzoTheme.colors.background).toBe('#ffffff');
  });

  it('exposes the syntax-highlight palette', () => {
    expect(kanzoTheme.colors.syntax.keyword).toBe('#7c3aed');
    expect(kanzoTheme.colors.syntax.string).toBe('#16a34a');
  });
});
