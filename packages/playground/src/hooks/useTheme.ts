/**
 * useTheme — resolve a `FossilThemeProp` ('light' | 'dark' | FossilTheme | undefined)
 * into:
 *   (a) the resolved `FossilTheme` shape (after normalising a string name),
 *       OR `undefined` when no theme is provided (host-provider model);
 *   (b) the flat CSS-variable record to apply at the playground root (via
 *       React's `style` prop) — EMPTY when no theme is provided;
 *   (c) a CodeMirror `Extension` that styles the editor host — a NO-OP
 *       extension when no theme is provided (the host's provider cascade
 *       reaches the CodeMirror DOM via the `--fossil-*` vars regardless).
 *
 * v0.2.x ownership model (per ADR-0035 — visual ownership separation):
 *   - No `theme` prop AND no ancestor provider → renders against browser
 *     defaults (the playground stays interactive; only the visual chrome
 *     cascades to defaults). This is the v0.2.x "OSS surface is
 *     brand-agnostic" contract.
 *   - Explicit `theme='light'` / `'dark'` → resolves to the built-in
 *     lightTheme / darkTheme exactly as v0.1.x (backwards compat invariant).
 *   - Explicit `FossilTheme` object → pass-through (advanced override path).
 *   - No `theme` prop BUT an ancestor `<KanzoThemeProvider/>` (or any other
 *     host-supplied theme cascade on a parent element) → the `--fossil-*`
 *     CSS variables flow down via the cascade; this hook produces empty
 *     cssVars (the host's provider already set them), and CodeMirror reads
 *     the same variables for its inner chrome.
 *
 * Why no-injection on undefined (vs auto-applying lightTheme):
 *   - In v0.1.x the default was 'light'. In v0.2.0 the v0.1 default was
 *     briefly flipped to 'fossil-ide' (plan 10-06); plan 10-09 reverts that
 *     default-flip per ADR-0035 (the brand surface lives in @kanzo/theme,
 *     not @fossil-lang/playground).
 *   - The v0.2.x contract is "OSS surface is brand-agnostic; hosts provide
 *     the brand cascade". Auto-applying lightTheme would re-introduce a
 *     brand decision the OSS surface shouldn't make.
 *   - v0.1.x consumers that explicitly passed `theme='light'` see ZERO
 *     change; they pass through the resolution branch as before.
 *
 * Consumer pattern unchanged from v0.1.x:
 *
 *   const { cssVars, editorTheme } = useTheme(themeProp);
 *   const extensions = useMemo(() => [...baseExts, editorTheme], [baseExts, editorTheme]);
 *   return <div style={cssVarsToStyle(cssVars)}>...</div>;
 *
 * @see decisions/0035-visual-ownership-separation.md
 */

import { useMemo } from 'react';
import type { Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';
import type { FossilTheme, FossilThemeProp } from '@fossil-lang/types';
import { lightTheme } from '../theme/light.js';
import { darkTheme } from '../theme/dark.js';
import { themeToCssVars } from '../theme/tokens.js';

export interface UseThemeResult {
  /** The resolved `FossilTheme` (after normalising a 'light'|'dark' string),
   *  OR `undefined` when no theme prop was passed — host-provider model
   *  per ADR-0035. */
  theme: FossilTheme | undefined;
  /** Flat CSS variable record. Apply to the playground root via React's
   *  `style` prop (use `cssVarsToStyle` from `theme/tokens` for the cast).
   *  EMPTY (`{}`) when no theme prop was passed — host supplies via Provider
   *  cascade per ADR-0035. */
  cssVars: Record<string, string>;
  /** CodeMirror Extension binding the resolved theme into the editor host
   *  + syntactic highlighter. A NO-OP extension (`EditorView.theme({})`)
   *  when no theme prop was passed — CodeMirror reads the cascaded
   *  `--fossil-*` vars from the host's provider directly via inline-style
   *  cascade. Memo-stable per `prop` identity. */
  editorTheme: Extension;
}

/**
 * Resolve a theme prop into the application bundle (CSS vars + CodeMirror
 * extension). Memoised on `prop` identity — built-in names ('light' | 'dark')
 * are referentially stable; custom `FossilTheme` objects should be
 * `useMemo`'d by the consumer to avoid editor-rebuilds on parent re-renders.
 *
 * Per ADR-0035 (visual ownership separation): the v0.2.x default behaviour
 * (no `prop` passed) returns empty cssVars + a no-op editor theme. The host
 * supplies the `--fossil-*` cascade via its own ThemeProvider (e.g.
 * `<KanzoThemeProvider/>` from `@kanzo/theme` for kanzo-branded hosts).
 *
 * Explicit `'light'` / `'dark'` / `FossilTheme` props behave exactly as
 * v0.1.x (backwards-compat invariant).
 */
export function useTheme(prop?: FossilThemeProp): UseThemeResult {
  return useMemo<UseThemeResult>(() => {
    // No prop → no injection. Host supplies via Provider cascade
    // (ADR-0035). Returning empty cssVars + a no-op editor theme keeps the
    // playground interactive against browser defaults; the host's
    // `<KanzoThemeProvider/>` (or equivalent) supplies `--fossil-*` vars
    // via inline-style on a wrapping div, which cascade into the
    // playground root + into CodeMirror's DOM via standard CSS.
    if (prop === undefined) {
      return {
        theme: undefined,
        cssVars: {},
        editorTheme: EditorView.theme({}),
      };
    }
    const theme: FossilTheme =
      typeof prop === 'string'
        ? prop === 'dark'
          ? darkTheme
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
