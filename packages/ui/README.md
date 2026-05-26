# @fossil-lang/ui

Radix UI primitives + Fossil design tokens for the `@fossil-lang/*` React
component family. CSS-variable driven; **no Tailwind, no shadcn**, no
build-time CSS authoring required of consumers.

## What this package provides

- **Primitives**: thin wrappers over `@radix-ui/react-*` (Tabs, Dialog,
  DropdownMenu, Tooltip, ScrollArea, Resizable, Toggle, Separator) — landed
  in Phase 10 plans 10-03 / 10-04 / 10-05.
- **Tokens**: the IDE-grade Fossil token surface (radii, spacing, motion,
  focus rings, control sizes) consumed by primitives via CSS custom
  properties — landed in Phase 10 plan 10-02.
- **`cx()`**: a 5-line class-name joiner replacing Keasy's `cn()` (clsx +
  tailwind-merge); we have no Tailwind utility collisions to resolve.
- **`fossil-ide` default theme**: landed in Phase 10 plan 10-06; auto-applied
  by `<FossilPlayground/>` when no `theme` prop is passed.

## Why a separate package?

`@fossil-lang/types` is explicitly zero-runtime (TypeScript `.d.ts` only).
Injecting React + Radix into it would break that contract. Sub-exporting
from `@fossil-lang/playground` would create dep cycles with the future
`@fossil-lang/editor` (Phase 11) + `@fossil-lang/viewer` (Phase 12).

Rationale + alternatives evaluated: see
[ADR-0033](../../decisions/0033-fossil-lang-ui-package.md).

## CSS variable naming

CSS custom property names are emitted by a mechanical flatten of the
`FossilTheme` shape (defined in `@fossil-lang/types`). Leaf string at
object path `[a, b, c]` becomes `--fossil-a-b-c`. **No camelCase leaves**;
multi-word names use sub-object nesting (`motion.duration.fast` →
`--fossil-motion-duration-fast`).

Rules + forbidden patterns: see
[ADR-0034](../../decisions/0034-css-variable-naming-mechanical-flatten.md).

## Status

**Phase 10 plan 10-01 — scaffold only.** Empty barrel, one cx utility,
one smoke test, 50 KB gzipped budget locked.

Roadmap:

| Plan  | Lands                                                 |
| ----- | ----------------------------------------------------- |
| 10-01 | This scaffold (you are here)                          |
| 10-02 | IDE-grade tokens + `FossilTheme` namespace extensions |
| 10-03 | Tabs + Dialog + Separator + shared `inject.ts`        |
| 10-04 | DropdownMenu + Tooltip + ScrollArea                   |
| 10-05 | Resizable + Toggle / ToggleGroup                      |
| 10-06 | `fossil-ide` default theme + playground re-export     |
| 10-07 | Multi-host-fixture styleguide page                    |
| 10-08 | Bundle assertion + WASM gate + phase close            |

## License

Apache-2.0
