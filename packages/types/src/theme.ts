/**
 * Semantic theme tokens for the Fossil playground. Hosts can override per-token
 * via CSS custom property cascade (e.g., setting `--fossil-color-background: #000`
 * on a parent element) OR via the `theme` prop accepting a `FossilTheme` shape.
 *
 * Built-in themes: 'light' (default) + 'dark'. See `@fossil-lang/playground` for
 * the actual values; this package only defines the SHAPE so packages downstream
 * (codemirror-fossil for the editor theme; playground for the panel theme) can
 * type their props consistently.
 */
export interface FossilTheme {
  /** Semantic colour tokens. CSS variable names: `--fossil-color-<key>`. */
  colors: {
    background: string;
    foreground: string;
    muted: string;
    border: string;
    accent: string;
    error: string;
    warning: string;
    info: string;
    /** NEW (Phase 10 VIS-03). Focus ring colour. Defaults to colors.accent
     *  in built-in themes. Hosts override to decouple ring colour from accent
     *  colour (a common pattern — Tailwind shadcn separates `--ring` from
     *  `--primary`). Emits `--fossil-colors-ring`. */
    ring: string;
    /** Editor-specific (syntactic highlighting categories). */
    syntax: {
      keyword: string;
      string: string;
      number: string;
      comment: string;
      identifier: string;
      operator: string;
      punctuation: string;
      /** Prefixed-name (e.g., `ex:foo`). */
      prefixedName: string;
      /** Absolute IRI literal. */
      iri: string;
    };
  };
  /** Font family + sizes. CSS variable names: `--fossil-font-<key>`. */
  fonts: {
    sans: string;
    mono: string;
    sizeBase: string;
    sizeSmall: string;
  };
  /** Spacing scale (CSS variable names: `--fossil-space-<n>`). */
  space: {
    xs: string;
    sm: string;
    md: string;
    lg: string;
    xl: string;
  };
  /** Border-radius scale. */
  radius: {
    sm: string;
    md: string;
  };
  /**
   * IDE-grade radii — extended from `radius` (v0.1 legacy, kept untouched
   * per ADR-0034 rule 6). Aligned with Tailwind defaults. CSS variable names:
   * `--fossil-radii-{sm,md,lg,xl,full}`. Phase 10 VIS-03.
   */
  radii: {
    /** 4px — small (input borders, badges). */
    sm: string;
    /** 6px — default (buttons, tab triggers). Matches Keasy `rounded-md`. */
    md: string;
    /** 8px — medium (cards). */
    lg: string;
    /** 12px — large (modals, dropdown menus). */
    xl: string;
    /** 9999px — pill / fully rounded. */
    full: string;
  };
  /**
   * IDE-grade spacing scale — extended from `space` (v0.1 legacy, kept
   * untouched). 7-step scale Tailwind-aligned. Numeric-string keys per
   * ADR-0034 rule 3. CSS variable names: `--fossil-spacing-{0,1,2,3,4,6,8}`.
   * Phase 10 VIS-03.
   */
  spacing: {
    /** 0 — collapse. Emits `--fossil-spacing-0`. */
    '0': string;
    /** 4px — tight padding (icon-only buttons). Emits `--fossil-spacing-1`. */
    '1': string;
    /** 8px — default horizontal padding (tab triggers — Keasy `px-2`). */
    '2': string;
    /** 12px. */
    '3': string;
    /** 16px — section padding. */
    '4': string;
    /** 24px — large gap. */
    '6': string;
    /** 32px — extra-large gap. */
    '8': string;
  };
  /**
   * Motion tokens — durations + easings. Per ADR-0034 rule 1 (sub-object
   * nesting for multi-word CSS var names). CSS variable names:
   * `--fossil-motion-duration-fast`, `--fossil-motion-duration-base`,
   * `--fossil-motion-easing`. Phase 10 VIS-03.
   */
  motion: {
    duration: {
      /** 150ms — fast hover/focus transitions. */
      fast: string;
      /** 200ms — default state transitions. */
      base: string;
    };
    /** Standard easing for material-style motion. */
    easing: string;
  };
  /**
   * Focus tokens. Per ADR-0034 rule 4: focus.ring is a PRE-RESOLVED rgba
   * shadow expression (NOT color-mix — Safari < 16.2 compat). Hosts override
   * `--fossil-focus-ring` directly to swap the full expression. The colour
   * anchor is `colors.ring`; this slot holds the composed box-shadow value.
   * CSS variable name: `--fossil-focus-ring`. Phase 10 VIS-03.
   */
  focus: {
    /** Composed focus-ring CSS expression. Example: '0 0 0 3px rgba(59, 130, 246, 0.5)'. */
    ring: string;
  };
  /**
   * Control sizes — heights for tabs / buttons / inputs / control surfaces.
   * Per ADR-0034 rule 7: the canonical default height is named `base` (not
   * an unsuffixed leaf — the flattener cannot emit both a leaf AND children
   * at the same path). CSS variable names:
   * `--fossil-size-control-base` (DEFAULT — primitives reference this),
   * `--fossil-size-control-sm`, `--fossil-size-control-lg`. Phase 10 VIS-03.
   */
  size: {
    control: {
      /** 32px — DEFAULT (Keasy `h-8`). Primitives reference this. */
      base: string;
      /** 24px — small (Keasy `h-6`). */
      sm: string;
      /** 40px — large (Keasy `h-10`). */
      lg: string;
    };
  };
}

/** Built-in @fossil-lang/* theme names. Hosts wanting brand-specific looks
 *  (e.g. the kanzo IDE look) wrap their tree in a brand-owned ThemeProvider —
 *  see ADR-0035 for the visual ownership separation story.
 *
 *  The OSS @fossil-lang/* surface is brand-agnostic: when no `theme` prop is
 *  passed AND no ancestor provider supplies the `--fossil-*` CSS-var cascade,
 *  consumers render against browser-default fallbacks (ARIA + keyboard + state
 *  selectors still wire correctly via the primitives; only the visual chrome
 *  cascades to defaults).
 *
 *  v0.1.x consumers passing 'light' or 'dark' explicitly see IDENTICAL
 *  behaviour. */
export type FossilThemeName = 'light' | 'dark';

/** Accepted value for the `theme` prop on `<FossilPlayground />` + sub-components. */
export type FossilThemeProp = FossilThemeName | FossilTheme;
