# @fossil-lang/ui

Radix UI primitives + Fossil CSS-variable contract for the `@fossil-lang/*`
React component family. **Theme-less** — primitives consume host-provided
`--fossil-*` CSS custom properties at runtime. No Tailwind, no shadcn peer
dependencies, no build-time CSS authoring required of consumers.

Part of the Fossil v0.2 milestone — adopted by `@fossil-lang/editor`
(Phase 11), `@fossil-lang/viewer` (Phase 12), and `@fossil-lang/playground`
composition refactor (Phase 14). Brand visuals are owned by the downstream
host/brand layer, not by `@fossil-lang/*`, per
[ADR-0035](../../decisions/0035-visual-ownership-separation.md).

## Install

```bash
pnpm add @fossil-lang/ui react react-dom
```

- **React 18+ or 19+** required (peer dependency).
- Radix individual `@radix-ui/react-*` packages are bundled into the
  published surface — no extra install needed.
- `react-resizable-panels` is bundled too (it powers the Resizable
  primitive; Radix has no resizable primitive — the single documented
  non-Radix exception per ADR-0033).

## Quick usage

```tsx
'use client';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@fossil-lang/ui';

export function MyEditor() {
  return (
    <Tabs defaultValue="mapping">
      <TabsList>
        <TabsTrigger value="mapping">Mapping</TabsTrigger>
        <TabsTrigger value="source">Source</TabsTrigger>
      </TabsList>
      <TabsContent value="mapping">…</TabsContent>
      <TabsContent value="source">…</TabsContent>
    </Tabs>
  );
}
```

The primitives render with browser-default fallbacks if nothing supplies the
`--fossil-*` CSS variables. For a branded look, define those variables on a
parent element (e.g. `:root`) — see the Theming story below.

## Primitives surface

| Primitive | Components | Notes |
|-----------|------------|-------|
| Tabs | `Tabs`, `TabsList`, `TabsTrigger`, `TabsContent` | `variant='default' \| 'line'` |
| Dialog | `Dialog`, `DialogTrigger`, `DialogPortal`, `DialogOverlay`, `DialogContent`, `DialogTitle`, `DialogDescription`, `DialogClose` | 8 subcomponents replacing direct `@radix-ui/react-dialog` usage |
| DropdownMenu | `DropdownMenu`, `DropdownMenuTrigger`, `DropdownMenuPortal`, `DropdownMenuGroup`, `DropdownMenuContent`, `DropdownMenuItem`, `DropdownMenuCheckboxItem`, `DropdownMenuRadioGroup`, `DropdownMenuRadioItem`, `DropdownMenuLabel`, `DropdownMenuSeparator`, `DropdownMenuSub`, `DropdownMenuSubTrigger`, `DropdownMenuSubContent` | Toolbar dropdowns; auto-Portal `Content` + `SubContent` |
| Tooltip | `TooltipProvider`, `Tooltip`, `TooltipTrigger`, `TooltipPortal`, `TooltipContent` | Hover diagnostics; Provider defaults `delayDuration=300`, `skipDelayDuration=100` |
| ScrollArea | `ScrollArea`, `ScrollBar` | Visible-on-hover scrollbars; `type='always'` forces always-mount |
| Resizable | `ResizablePanelGroup`, `ResizablePanel`, `ResizableHandle` | Built on `react-resizable-panels`; `withHandle` prop renders grip indicator |
| Toggle | `Toggle`, `ToggleGroup`, `ToggleGroupItem` | `variant='default' \| 'outline'`; `ToggleGroupItem` reuses `data-slot="toggle"` |
| Separator | `Separator` | Orientation-aware horizontal + vertical dividers |

Plus utilities: `cx` (class-name joiner), `injectFossilUiStyles`
(singleton stylesheet helper — called automatically when any primitive
mounts), and the `ClassValue`, `TabsListVariant`, `TabsListProps`,
`ToggleVariant`, `ToggleProps`, `ToggleGroupProps`, `ToggleGroupItemProps`,
`ResizableHandleProps` type exports.

## CSS variable contract (per ADR-0034)

All primitives style themselves via CSS custom properties under the
`--fossil-*` prefix. Names emit by **mechanical flatten** of the
`FossilTheme` shape (defined in `@fossil-lang/types`): leaf string at
object path `[a, b, c]` becomes `--fossil-a-b-c`. **No camelCase leaves**;
multi-word names use sub-object nesting.

Hosts may override any token via the CSS cascade — no JS prop required.
This is the public contract: as long as the host provides values for
these variables on an ancestor element, the primitives render with the
intended look.

### Colors

`--fossil-colors-background`, `--fossil-colors-foreground`,
`--fossil-colors-muted`, `--fossil-colors-border`,
`--fossil-colors-accent`, `--fossil-colors-ring`,
`--fossil-colors-error`, `--fossil-colors-warning`,
`--fossil-colors-info`, plus `--fossil-colors-syntax-*` (the v0.1 syntax
palette).

`--fossil-colors-ring` is the focus-ring colour slot (new in Phase 10
plan 10-02). Defaults to the accent value but can be overridden
independently per Keasy convention (`--ring` vs `--primary` separate in
shadcn).

### Radii (Phase 10 VIS-03)

| Variable | Default | Use |
|---|---|---|
| `--fossil-radii-sm` | `4px` | small (input borders) |
| `--fossil-radii-md` | `6px` | default (buttons, tabs) |
| `--fossil-radii-lg` | `8px` | medium (cards) |
| `--fossil-radii-xl` | `12px` | large (modals, dropdowns) |
| `--fossil-radii-full` | `9999px` | pill |

(Coexists with the legacy `--fossil-radius-{sm,md}` from v0.1 — see
ADR-0034 rule 6 backwards-compat note.)

### Spacing scale (Phase 10 VIS-03)

`--fossil-spacing-0` (0), `--fossil-spacing-1` (4px),
`--fossil-spacing-2` (8px), `--fossil-spacing-3` (12px),
`--fossil-spacing-4` (16px), `--fossil-spacing-6` (24px),
`--fossil-spacing-8` (32px) — Tailwind-aligned so a host's existing
`--background`/`--ring`/etc. map cleanly.

### Motion

- `--fossil-motion-duration-fast` (150ms)
- `--fossil-motion-duration-base` (200ms)
- `--fossil-motion-easing` (`cubic-bezier(0.4, 0, 0.2, 1)`)

### Focus ring

`--fossil-focus-ring` carries the **entire** CSS expression as a single
pre-resolved value, e.g. `0 0 0 3px rgba(59, 130, 246, 0.5)`. No
`color-mix()` is used anywhere in the v0.2 baseline (Safari <16.2
compat per ADR-0034 rule 4). Hosts that want to swap the ring colour or
switch to `outline` override the entire expression in one assignment:

```css
:root { --fossil-focus-ring: 0 0 0 2px hotpink; }
```

### Control sizes

- `--fossil-size-control-sm` (24px)
- `--fossil-size-control-base` (32px) — default tab / button / input height
- `--fossil-size-control-lg` (40px)

(Named-leaf `base` instead of unsuffixed branch node — ADR-0034 rule 7
prevents the flattener from emitting both a leaf and children at the
same path.)

## Theming story (post-Phase 10)

`@fossil-lang/ui` is **theme-less**: it consumes the
`--fossil-*` CSS-var contract at runtime. The package does not ship a
default theme; it does not assume a brand. A consumer importing
primitives without any provider gets browser-default chrome with
working ARIA + keyboard + state plumbing (all provided by Radix
intrinsically); only the visual styling cascades from CSS vars the host
supplies.

### Supplying the variables

The brand/host layer owns the visuals and supplies the `--fossil-*`
variables — `@fossil-lang/*` ships none. The simplest path is a static
declaration on `:root` (or any wrapping element); every descendant
inherits via the CSS cascade:

```css
:root {
  --fossil-colors-accent: #3b82f6;
  --fossil-fonts-sizeBase: 13px;
  /* …the rest of the --fossil-* contract… */
}
```

Or emit them programmatically from a `FossilTheme` value — see "Bring
your own theme" below.

### Bring your own theme

Hosts assemble a `FossilTheme` value (or a partial CSS-var map)
themselves and emit `--fossil-*` variables on whatever DOM element they
prefer. The flattening from a `FossilTheme` object to CSS-var names
follows ADR-0034's mechanical-flatten contract (`colors.accent` →
`--fossil-colors-accent`, and so on).

### Why theme-less?

Mid-Phase 10 (plan 10-06 → plan 10-09) we briefly shipped a `fossil-ide`
default theme INSIDE `@fossil-lang/playground`. That coupled OSS Fossil
to one specific brand, inverting the proper ownership: WASM-first
architecture means the visual layer is the only host-coupling point, so
visuals must live in the brand layer, not the OSS toolchain layer. ADR-0035
records the correction; the default-flip was reverted and brand
ownership moved out of the OSS toolchain entirely — into the downstream
brand/product layer. See
[ADR-0035](../../decisions/0035-visual-ownership-separation.md) for
context, decision, and consequences.

## Why a separate package?

- `@fossil-lang/types` is explicitly zero-runtime (TypeScript `.d.ts`
  only). Injecting React + Radix into it would break that contract.
- Sub-exporting from `@fossil-lang/playground` would create dep cycles
  with the future `@fossil-lang/editor` (Phase 11) +
  `@fossil-lang/viewer` (Phase 12).
- A standalone package can be consumed by any host (editor, viewer,
  playground, Keasy frontend) without dragging the playground's run
  pipeline + permalink + DuckDB peers.

Rationale + alternatives evaluated: see
[ADR-0033](../../decisions/0033-fossil-lang-ui-package.md) (and the
Amendment 2026-05-26 section recording the theme-less invariant per
plan 10-09 / ADR-0035).

## Bundle budget

`@fossil-lang/ui` ships under 50 KB gzipped (cap enforced via
`size-limit` in CI). Phase 10 close measurement: **49.29 KB / 50 KB
(0.71 KB headroom)**. The published surface includes:

- 8 Radix individual `@radix-ui/react-*` packages (bundled, NOT peer)
- `react-resizable-panels` (the documented non-Radix exception)
- ~4.53 KB of our own wrapper code (measured via a diagnostic
  `.size-limit.cjs` entry that excludes the bundled deps — lets us
  track wrapper-growth independently of upstream Radix bumps)

Tree-shaking is preserved by importing individual `@radix-ui/react-*`
packages (not the umbrella `radix-ui` meta-package).

## Status

Phase 10 complete (Wave 4 — plan 10-08 / 10-09). All 8 primitives
shipped; CSS-variable contract documented + ADR-0034 codified; bundle
under cap. Phase 11 (editor extraction) and Phase 12 (viewer
extraction) consume this surface next.

## License

Apache-2.0 — same as the rest of the `@fossil-lang/*` family.
