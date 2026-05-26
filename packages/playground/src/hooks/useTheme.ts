/**
 * useTheme — resolve a `FossilThemeProp` ('light' | 'dark' | FossilTheme) into:
 *   (a) the resolved `FossilTheme` shape (after normalising a string name);
 *   (b) the flat CSS-variable record to apply at the playground root (via
 *       React's `style` prop);
 *   (c) a CodeMirror `Extension` that styles the editor host to match — both
 *       the surrounding chrome (background/foreground/gutter/cursor) AND the
 *       syntactic highlighting categories (keyword/string/number/comment/...).
 *
 * Why this hook (vs scattering theme reads across components):
 *   - SINGLE source of truth: change the variable, CodeMirror + the chrome
 *     restyle together. THEME-01's Monaco-style override API depends on this
 *     coherence — host overrides one variable, BOTH the editor and chrome
 *     pick up the change.
 *   - Memoisation: the editor theme Extension is structurally complex; we
 *     compute it once per `prop` identity so React 18 + StrictMode re-renders
 *     don't tear down + rebuild the CodeMirror editor (which is the most
 *     expensive child of `FossilPlayground`).
 *   - Inversion: the host's `theme` prop can be a string name OR a full
 *     `FossilTheme` object. Tools that want to override JUST 2-3 colours
 *     should spread the built-in: `{ ...lightTheme, colors: { ...lightTheme.colors, accent: '#ff00ff' } }`
 *     — this works thanks to the resolution being shallow at the API boundary
 *     + the flattener walking the resolved value.
 *
 * Consumer pattern:
 *
 *   const { cssVars, editorTheme } = useTheme(themeProp);
 *   const extensions = useMemo(() => [...baseExts, editorTheme], [baseExts, editorTheme]);
 *   return <div style={cssVarsToStyle(cssVars)}>...</div>;
 *
 * The editor theme uses `@lezer/highlight` tags (the canonical CodeMirror
 * highlight-tag vocabulary the StreamParser path emits as string names —
 * see `@fossil-lang/codemirror-fossil/src/tags.ts`'s `KIND_TO_TAG`). Themes
 * matching on `t.keyword` style ALL keyword tokens (`prefix`, `from`, `in`,
 * `use`, `as`, `and`, `or`, `not`, `iri`) uniformly.
 */

import { useMemo } from 'react';
import type { Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';
import type { FossilTheme, FossilThemeProp } from '@fossil-lang/types';
import { lightTheme } from '../theme/light.js';
import { darkTheme } from '../theme/dark.js';
import { fossilIdeTheme } from '../theme/fossil-ide.js';
import { themeToCssVars } from '../theme/tokens.js';

export interface UseThemeResult {
  /** The resolved `FossilTheme` (after normalising a 'light'|'dark' string). */
  theme: FossilTheme;
  /** Flat CSS variable record. Apply to the playground root via React's
   *  `style` prop (use `cssVarsToStyle` from `theme/tokens` for the cast). */
  cssVars: Record<string, string>;
  /** CodeMirror Extension binding the resolved theme into the editor host
   *  + syntactic highlighter. Memo-stable per `prop` identity. */
  editorTheme: Extension;
}

/**
 * Resolve a theme prop into the application bundle (CSS vars + CodeMirror
 * extension). Memoised on `prop` identity — built-in names
 * ('light' | 'dark' | 'fossil-ide') are referentially stable; custom
 * `FossilTheme` objects should be `useMemo`'d by the consumer to avoid
 * editor-rebuilds on parent re-renders.
 *
 * Default is `'fossil-ide'` (v0.2 onwards; v0.1.x default was `'light'` —
 * consumers passing prop explicitly see no change). Pass `'light'` or
 * `'dark'` for the v0.1.x built-ins; pass a `FossilTheme` object for a
 * custom palette (spread one of the built-ins to override a subset of
 * tokens).
 */
export function useTheme(
  prop: FossilThemeProp = 'fossil-ide',
): UseThemeResult {
  return useMemo<UseThemeResult>(() => {
    const theme: FossilTheme =
      typeof prop === 'string'
        ? prop === 'dark'
          ? darkTheme
          : prop === 'fossil-ide'
            ? fossilIdeTheme
            : lightTheme // fallback for 'light' AND any unknown string
        : prop;
    const cssVars = themeToCssVars(theme);
    const editorTheme = buildEditorTheme(theme);
    return { theme, cssVars, editorTheme };
  }, [prop]);
}

/**
 * Build the CodeMirror Extension pair (HighlightStyle + EditorView.theme)
 * that maps the resolved `FossilTheme` onto the editor host.
 *
 * Highlight tags chosen to match what the `@fossil-lang/codemirror-fossil`
 * StreamParser emits via `KIND_TO_TAG` (tags.ts) — `keyword`/`string`/
 * `number`/`comment`/`variableName`/`operator`/`punctuation`/`meta`. The
 * tag→colour mapping ties syntactic categories to the FossilTheme's
 * `colors.syntax.*` palette so a theme switch updates both the chrome AND
 * the highlighter in lockstep.
 */
function buildEditorTheme(theme: FossilTheme): Extension {
  const highlight = HighlightStyle.define([
    { tag: t.keyword, color: theme.colors.syntax.keyword, fontWeight: '600' },
    { tag: t.string, color: theme.colors.syntax.string },
    { tag: t.number, color: theme.colors.syntax.number },
    {
      tag: t.comment,
      color: theme.colors.syntax.comment,
      fontStyle: 'italic',
    },
    { tag: t.variableName, color: theme.colors.syntax.identifier },
    { tag: t.operator, color: theme.colors.syntax.operator },
    { tag: t.punctuation, color: theme.colors.syntax.punctuation },
    // `meta` is the canonical Lezer tag for language-machinery markers
    // (`@export`, `@dcat`, …) per codemirror-fossil/src/tags.ts.
    { tag: t.meta, color: theme.colors.syntax.prefixedName },
  ]);

  const themeExt = EditorView.theme({
    '&': {
      backgroundColor: theme.colors.background,
      color: theme.colors.foreground,
      fontSize: theme.fonts.sizeBase,
      fontFamily: theme.fonts.mono,
    },
    '.cm-content': {
      caretColor: theme.colors.accent,
    },
    '.cm-cursor, .cm-dropCursor': {
      borderLeftColor: theme.colors.accent,
    },
    // Selection background — use accent with reduced opacity. The trailing
    // '33' is hex for ≈20% alpha which gives a discoverable highlight without
    // hiding the text underneath.
    '.cm-selectionBackground, ::selection': {
      backgroundColor: theme.colors.accent + '33',
    },
    '.cm-gutters': {
      backgroundColor: theme.colors.background,
      color: theme.colors.muted,
      borderRight: `1px solid ${theme.colors.border}`,
    },
    '.cm-activeLineGutter': {
      backgroundColor: theme.colors.border,
    },
    // Focus ring — per WCAG 2.1 SC 2.4.7 (Focus Visible). The 2px outline at
    // accent colour ensures keyboard users can locate the editor focus on
    // either light or dark backgrounds (accent picks contrast against both).
    '&.cm-focused': {
      outline: `2px solid ${theme.colors.accent}`,
      outlineOffset: '-2px',
    },
  });

  return [syntaxHighlighting(highlight), themeExt];
}
