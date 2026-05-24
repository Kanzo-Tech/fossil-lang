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
}

/** Built-in theme names; `theme` prop also accepts a full `FossilTheme` shape. */
export type FossilThemeName = 'light' | 'dark';

/** Accepted value for the `theme` prop on `<FossilPlayground />` + sub-components. */
export type FossilThemeProp = FossilThemeName | FossilTheme;
