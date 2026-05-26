# @kanzo/theme

Kanzo brand tokens + `<KanzoThemeProvider/>` React component. The
v0.2-canonical brand-owned visual layer for kanzo hosts (apps/landing,
future Keasy migration consumers).

Per [ADR-0035](../../decisions/0035-visual-ownership-separation.md) — visual
ownership is separated from the @fossil-lang/* OSS surface: the OSS
primitives in `@fossil-lang/ui` are **theme-less** and consume the
`--fossil-*` CSS custom property contract at runtime. This package is the
brand-owned theme that supplies that contract for kanzo-branded hosts.

## Why a separate package?

In Phase 10 plan 10-06 the kanzo IDE-grade theme briefly lived inside
`@fossil-lang/playground` (as `fossilIdeTheme`, the default theme). This
coupled OSS Fossil to one specific brand. Plan 10-09 extracted the theme
into this `@kanzo/theme` package and reverted `@fossil-lang/playground`'s
defaults so the OSS surface is brand-agnostic again. Non-kanzo hosts ship
their own theme; kanzo-branded hosts wrap their tree in
`<KanzoThemeProvider/>`.

The full reasoning + alternatives evaluated:
[ADR-0035](../../decisions/0035-visual-ownership-separation.md).

## Installation

```bash
pnpm add @kanzo/theme
```

Peer dependencies: React 18+ / 19+, React DOM 18+ / 19+.

## Usage

Wrap your application root (or just the playground subtree) in
`<KanzoThemeProvider/>`. Every descendant — including `<FossilPlayground/>`,
standalone `@fossil-lang/ui` primitives, and your own kanzo-branded UI —
inherits the `--fossil-*` CSS-var cascade.

```tsx
import { KanzoThemeProvider } from '@kanzo/theme';
import { FossilPlayground } from '@fossil-lang/playground';

export function App() {
  return (
    <KanzoThemeProvider>
      <FossilPlayground resolver={resolver} wasmUrl={wasmUrl} />
    </KanzoThemeProvider>
  );
}
```

### Custom palette variant

Spread `kanzoTheme` and override the slots you care about. Memoise the
object to avoid recomputing the cssVars record on every render.

```tsx
import { useMemo } from 'react';
import { kanzoTheme, KanzoThemeProvider } from '@kanzo/theme';

const customTheme = useMemo(
  () => ({
    ...kanzoTheme,
    colors: { ...kanzoTheme.colors, accent: '#ff00ff' },
  }),
  [],
);

<KanzoThemeProvider theme={customTheme}>...</KanzoThemeProvider>
```

### Reading the theme programmatically

For consumers that want to read the active theme + flattened cssVars
(e.g. a custom palette swatch, a brand-aware screenshot tool):

```tsx
import { useKanzoTheme } from '@kanzo/theme';

function PaletteSwatch() {
  const { theme, cssVars } = useKanzoTheme();
  return <div>Accent: {theme.colors.accent}</div>;
}
```

`useKanzoTheme()` throws if called outside a `<KanzoThemeProvider/>` — the
brand surface is opt-in and consumers that haven't installed it shouldn't
reach for the hook.

## CSS variable contract

Token names follow [ADR-0034](../../decisions/0034-css-variable-naming-mechanical-flatten.md)
— a leaf string at object path `[a, b, c]` in the `FossilTheme` shape becomes
`--fossil-a-b-c`. Highlights:

- `--fossil-colors-{background, foreground, accent, ring, ...}`
- `--fossil-fonts-{sans, mono, sizeBase, sizeSmall}` — kanzo's IDE-grade
  signature is `sizeBase: 13px` + `sizeSmall: 11px`
- `--fossil-radii-{sm, md, lg, xl, full}` — IDE-grade scale
- `--fossil-spacing-{0, 1, 2, 3, 4, 6, 8}` — Tailwind-aligned
- `--fossil-motion-duration-{fast, base}` + `--fossil-motion-easing`
- `--fossil-focus-ring` — pre-resolved rgba box-shadow (Safari <16.2 compat)
- `--fossil-size-control-{base, sm, lg}`

Hosts can override ANY `--fossil-*` token at ANY ancestor element via plain
CSS — the cascade wins, so per-instance overrides require no prop changes.

## Relationship to @fossil-lang/ui

`@fossil-lang/ui` (the OSS primitive package; Radix wrappers + cx utility)
is theme-less. Its components reference `--fossil-*` CSS custom properties
at runtime via inline `style={{ background: 'var(--fossil-colors-background)' }}`
patterns. Without a theme provider supplying those vars, the primitives
render against the browser-default fallback values (Radix's `data-state`
selectors still wire ARIA + keyboard correctly; only the visual chrome
falls back).

This package is one such theme provider. Other brands ship their own.

## License

Apache-2.0 — same as the @fossil-lang/* OSS family.
