/**
 * THEME-01 unit tests — the `theme` prop on `<FossilPlayground/>` resolves
 * 'light' | 'dark' | FossilTheme into CSS variables applied at the
 * playground root.
 *
 * Coverage split:
 *   - Default theme ('light' when prop omitted)
 *   - 'dark' built-in
 *   - Custom FossilTheme override (proves the FossilTheme shape passes through)
 *   - WCAG 2.1 AA contrast assertion on the built-in foreground/background
 *     pairs (a representative subset — the full WCAG gate runs in 08-11
 *     via @axe-core/playwright)
 *
 * Test scope:
 *   The component renders + the root carries the expected `--fossil-*`
 *   custom properties on its inline style. We do NOT cross-check against
 *   the CodeMirror editor's inner DOM (happy-dom doesn't compute layout
 *   sufficiently to verify the cascade through CodeMirror's shadow-style
 *   class names — that's an apps/landing/ Playwright test).
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import {
  FossilPlayground,
  lightTheme,
  darkTheme,
  themeToCssVars,
} from '../src/index.js';
import { createMockResolver } from '@fossil-lang/resolvers';

const mockResolver = createMockResolver({
  fixtures: {},
  connectors: [{ name: 'examples', type: 'examples' as const }],
});

/**
 * Compute the relative luminance of a `#rrggbb` colour per WCAG 2.1
 * Understanding 1.4.3. Used to assert the built-in themes hit the
 * 4.5:1 body-text contrast ratio against their backgrounds.
 *
 * Inlined here (no `wcag-contrast` dep) because the formula is small,
 * stable, and avoids pulling a runtime dep into the test path.
 */
function luminance(hex: string): number {
  const m = /^#([0-9a-f]{6})$/i.exec(hex);
  if (!m) throw new Error(`luminance: expected #rrggbb, got ${hex}`);
  const r = parseInt(m[1].slice(0, 2), 16) / 255;
  const g = parseInt(m[1].slice(2, 4), 16) / 255;
  const b = parseInt(m[1].slice(4, 6), 16) / 255;
  const channel = (c: number): number =>
    c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG contrast ratio between two `#rrggbb` colours. AA body text ≥ 4.5:1. */
function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  const [lighter, darker] = la > lb ? [la, lb] : [lb, la];
  return (lighter + 0.05) / (darker + 0.05);
}

describe('THEME-01: <FossilPlayground theme={...}/>', () => {
  it('default (no theme prop) applies light CSS vars at the root', () => {
    const { container } = render(
      <FossilPlayground resolver={mockResolver} wasmUrl="https://mock/x" />,
    );
    const root = container.querySelector(
      '.fossil-playground',
    ) as HTMLElement | null;
    expect(root).toBeTruthy();
    // Inline style carries the flattened FossilTheme as CSS vars.
    expect(root!.style.getPropertyValue('--fossil-colors-background')).toBe(
      lightTheme.colors.background,
    );
    expect(root!.style.getPropertyValue('--fossil-colors-foreground')).toBe(
      lightTheme.colors.foreground,
    );
  });

  it('theme="dark" applies dark CSS vars', () => {
    const { container } = render(
      <FossilPlayground
        resolver={mockResolver}
        wasmUrl="https://mock/x"
        theme="dark"
      />,
    );
    const root = container.querySelector(
      '.fossil-playground',
    ) as HTMLElement | null;
    expect(root).toBeTruthy();
    expect(root!.style.getPropertyValue('--fossil-colors-background')).toBe(
      darkTheme.colors.background,
    );
    expect(root!.style.getPropertyValue('--fossil-colors-foreground')).toBe(
      darkTheme.colors.foreground,
    );
    // Syntax-leaf depth proves the flattener walks the full nested shape.
    expect(
      root!.style.getPropertyValue('--fossil-colors-syntax-keyword'),
    ).toBe(darkTheme.colors.syntax.keyword);
  });

  it('a custom FossilTheme overrides individual tokens', () => {
    const custom = {
      ...lightTheme,
      colors: { ...lightTheme.colors, background: '#ff00ff', accent: '#abcdef' },
    };
    const { container } = render(
      <FossilPlayground
        resolver={mockResolver}
        wasmUrl="https://mock/x"
        theme={custom}
      />,
    );
    const root = container.querySelector(
      '.fossil-playground',
    ) as HTMLElement | null;
    expect(root).toBeTruthy();
    // Overridden tokens take effect.
    expect(root!.style.getPropertyValue('--fossil-colors-background')).toBe(
      '#ff00ff',
    );
    expect(root!.style.getPropertyValue('--fossil-colors-accent')).toBe(
      '#abcdef',
    );
    // Un-overridden tokens fall through from the spread base.
    expect(root!.style.getPropertyValue('--fossil-colors-foreground')).toBe(
      lightTheme.colors.foreground,
    );
  });

  it('the flat CSS-var record from themeToCssVars matches a key sample', () => {
    const vars = themeToCssVars(lightTheme);
    expect(vars['--fossil-colors-background']).toBe(
      lightTheme.colors.background,
    );
    expect(vars['--fossil-colors-syntax-keyword']).toBe(
      lightTheme.colors.syntax.keyword,
    );
    expect(vars['--fossil-fonts-mono']).toBe(lightTheme.fonts.mono);
    expect(vars['--fossil-space-md']).toBe(lightTheme.space.md);
    expect(vars['--fossil-radius-md']).toBe(lightTheme.radius.md);
  });

  it('built-in light theme meets WCAG 2.1 AA body-text contrast (foreground vs background)', () => {
    const ratio = contrast(
      lightTheme.colors.foreground,
      lightTheme.colors.background,
    );
    // AA body text floor is 4.5:1; light theme should be well above (≈15:1).
    expect(ratio).toBeGreaterThanOrEqual(4.5);
  });

  it('built-in dark theme meets WCAG 2.1 AA body-text contrast (foreground vs background)', () => {
    const ratio = contrast(
      darkTheme.colors.foreground,
      darkTheme.colors.background,
    );
    expect(ratio).toBeGreaterThanOrEqual(4.5);
  });

  it('built-in light theme error/info/warning each meet AA contrast against the background', () => {
    // Error/info/warning are status indicators — they MUST be readable as
    // body text on the chrome background (4.5:1 AA floor).
    expect(
      contrast(lightTheme.colors.error, lightTheme.colors.background),
    ).toBeGreaterThanOrEqual(4.5);
    expect(
      contrast(lightTheme.colors.info, lightTheme.colors.background),
    ).toBeGreaterThanOrEqual(4.5);
    expect(
      contrast(lightTheme.colors.warning, lightTheme.colors.background),
    ).toBeGreaterThanOrEqual(4.5);
  });
});

/**
 * Phase 10 VIS-03 — new token namespaces flatten to CSS variables per
 * ADR-0034 (mechanical-flatten naming convention).
 *
 * These assertions are SEPARATE from the THEME-01 suite above so the
 * v0.1.x consumer regression surface stays scoped (the THEME-01 tests
 * remain a stable contract; this suite adds NEW guarantees on top).
 */
describe('Phase 10 VIS-03 — new token namespaces flatten to CSS variables (ADR-0034)', () => {
  const lightVars = themeToCssVars(lightTheme);
  const darkVars = themeToCssVars(darkTheme);

  describe('colors.ring (new slot)', () => {
    it('emits --fossil-colors-ring as a sibling of --fossil-colors-accent', () => {
      expect(lightVars['--fossil-colors-ring']).toBe('#3b82f6');
      expect(darkVars['--fossil-colors-ring']).toBe('#60a5fa');
      // Default == accent for built-in themes (hosts override to decouple).
      expect(lightVars['--fossil-colors-ring']).toBe(
        lightVars['--fossil-colors-accent'],
      );
      expect(darkVars['--fossil-colors-ring']).toBe(
        darkVars['--fossil-colors-accent'],
      );
    });
  });

  describe('radii namespace', () => {
    it('emits --fossil-radii-{sm,md,lg,xl,full} from light theme', () => {
      expect(lightVars['--fossil-radii-sm']).toBe('4px');
      expect(lightVars['--fossil-radii-md']).toBe('6px');
      expect(lightVars['--fossil-radii-lg']).toBe('8px');
      expect(lightVars['--fossil-radii-xl']).toBe('12px');
      expect(lightVars['--fossil-radii-full']).toBe('9999px');
    });

    it('dark theme emits identical radii values (theme-independent)', () => {
      expect(darkVars['--fossil-radii-md']).toBe('6px');
      expect(darkVars['--fossil-radii-full']).toBe('9999px');
    });
  });

  describe('spacing namespace', () => {
    it('emits --fossil-spacing-{0,1,2,3,4,6,8} (numeric-string keys, ADR-0034 rule 3)', () => {
      expect(lightVars['--fossil-spacing-0']).toBe('0');
      expect(lightVars['--fossil-spacing-1']).toBe('4px');
      expect(lightVars['--fossil-spacing-2']).toBe('8px');
      expect(lightVars['--fossil-spacing-3']).toBe('12px');
      expect(lightVars['--fossil-spacing-4']).toBe('16px');
      expect(lightVars['--fossil-spacing-6']).toBe('24px');
      expect(lightVars['--fossil-spacing-8']).toBe('32px');
    });
  });

  describe('motion namespace (sub-object nesting, ADR-0034 rule 1)', () => {
    it('emits --fossil-motion-duration-{fast,base} + --fossil-motion-easing', () => {
      expect(lightVars['--fossil-motion-duration-fast']).toBe('150ms');
      expect(lightVars['--fossil-motion-duration-base']).toBe('200ms');
      expect(lightVars['--fossil-motion-easing']).toMatch(/cubic-bezier/);
    });

    it('does NOT emit camelCase variants like --fossil-motion-durationFast', () => {
      // ADR-0034 rule 1 forbids camelCase leaves — the mechanical flattener
      // would emit them verbatim, breaking the kebab-case CSS-var convention.
      // Sub-object nesting (motion.duration.fast) is the antidote.
      expect(lightVars['--fossil-motion-durationFast']).toBeUndefined();
      expect(lightVars['--fossil-motion-durationBase']).toBeUndefined();
    });
  });

  describe('focus.ring (pre-resolved rgba, ADR-0034 rule 4)', () => {
    it('emits --fossil-focus-ring as a pre-resolved rgba shadow', () => {
      expect(lightVars['--fossil-focus-ring']).toBe(
        '0 0 0 3px rgba(59, 130, 246, 0.5)',
      );
      expect(darkVars['--fossil-focus-ring']).toBe(
        '0 0 0 3px rgba(96, 165, 250, 0.5)',
      );
    });

    it('focus ring value does NOT use modern CSS colour-blending functions (Safari <16.2 compat)', () => {
      // ADR-0034 rule 4: the baseline must work on Safari <16.2, which lacks
      // color-mix(). Hosts can opt into modern functions via override.
      expect(lightVars['--fossil-focus-ring']).not.toMatch(/color-mix/);
      expect(darkVars['--fossil-focus-ring']).not.toMatch(/color-mix/);
    });

    it('does NOT emit --fossil-focusRing (camelCase forbidden per ADR-0034)', () => {
      expect(lightVars['--fossil-focusRing']).toBeUndefined();
    });
  });

  describe('size.control namespace (sub-object nesting, ADR-0034 rule 7)', () => {
    it('emits --fossil-size-control-{base,sm,lg}', () => {
      expect(lightVars['--fossil-size-control-base']).toBe('32px');
      expect(lightVars['--fossil-size-control-sm']).toBe('24px');
      expect(lightVars['--fossil-size-control-lg']).toBe('40px');
    });

    it('does NOT emit unsuffixed --fossil-size-control (rule 7 — flattener emits leaves, not branch nodes)', () => {
      // The default leaf is named `base`, so the canonical default control
      // height is `--fossil-size-control-base`. Branch nodes never emit.
      expect(lightVars['--fossil-size-control']).toBeUndefined();
    });
  });

  describe('backwards compat — existing v0.1 tokens not regressed (ADR-0034 rule 6)', () => {
    it('still emits --fossil-colors-* / --fossil-fonts-* / --fossil-space-* / --fossil-radius-*', () => {
      expect(lightVars['--fossil-colors-background']).toBe('#ffffff');
      expect(lightVars['--fossil-fonts-mono']).toBeDefined();
      expect(lightVars['--fossil-space-md']).toBe('16px');
      expect(lightVars['--fossil-radius-sm']).toBe('4px');
      expect(lightVars['--fossil-radius-md']).toBe('8px');
    });

    it('dark theme also preserves existing v0.1 tokens', () => {
      expect(darkVars['--fossil-colors-background']).toBe('#0f172a');
      expect(darkVars['--fossil-space-md']).toBe('16px');
      expect(darkVars['--fossil-radius-md']).toBe('8px');
    });
  });
});
