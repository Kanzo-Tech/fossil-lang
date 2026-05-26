# ADR 0033: Vendor Radix primitives + Fossil tokens in a NEW `@fossil-lang/ui` package, not as a sub-export of `@fossil-lang/playground` or addition to `@fossil-lang/types`

**Date:** 2026-05-26
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** .planning/phases/10-visual-foundation-radix-ide-theme/10-CONTEXT.md (decisions block) — milestone-initiation conversation 2026-05-26

## Context

v0.2 of the `@fossil-lang/*` family ships two new packages (`@fossil-lang/editor`
in Phase 11 + `@fossil-lang/viewer` in Phase 12) that both need a shared
IDE-style visual layer — Radix primitives (Tabs, Dialog, DropdownMenu, Tooltip,
ScrollArea, Resizable, Toggle, Separator) and an expanded token set (radii,
spacing, motion, focus rings, control sizes). The existing
`@fossil-lang/playground` v0.1.x ships a partial token set (THEME-01 from
Phase 8 plan 08-10) but NO primitives — `BibtexModal` uses
`@radix-ui/react-dialog` directly without a wrapper.

Three placement options for the shared visual layer:

1. **Sub-export of `@fossil-lang/playground`** — add a `primitives/`
   directory and re-export. Pro: zero new package. Con: `@fossil-lang/editor`
   + `@fossil-lang/viewer` would have to depend on the entire playground
   (which itself will depend on editor + viewer once Phase 14 lands — cycle).
   Hard cycle, rejected.

2. **Addition to `@fossil-lang/types`** — extend it with the primitives.
   Pro: already a peer of editor + viewer. Con: `@fossil-lang/types` is
   explicitly zero-runtime per its `package.json`. Injecting React + Radix
   breaks the contract.

3. **New `@fossil-lang/ui` package** — separate package, peer-depended by
   editor + viewer + playground. Pro: clean dependency direction (ui → types;
   editor → ui + types; viewer → ui + types; playground → ui + editor +
   viewer + types — DAG, no cycles). Adds one more package to the publish
   set (9 total in v0.2.0). Con: another publish target.

Bundle cost analysis: 8 primitives × ~6 KB avg (Radix individual packages)
= ~48 KB gzipped. Plus tokens (~2 KB). Stays under the 50 KB Phase 10 budget
per CONTEXT.md. Tree-shaking matters — each primitive is imported
individually from `@radix-ui/react-tabs`, `@radix-ui/react-dialog`, etc. (NOT
the umbrella `radix-ui` meta-package, which has worse tree-shaking).

## Decision

We will create a NEW workspace package `@fossil-lang/ui` (option 3) under
`packages/ui/` with:

- `peerDependencies`: React 18+/19+ + React DOM
- `dependencies`: individual `@radix-ui/react-*` packages per primitive
  (added in Phase 10 plans 10-03/04/05)
- A `cx()` utility (~5 lines) replacing Keasy's `cn()` (clsx +
  tailwind-merge) — no Tailwind utility collisions to resolve
- Tokens (FossilTheme namespaces) live in `@fossil-lang/types`; the COMPUTED
  CSS-variable record is produced by `themeToCssVars` in
  `@fossil-lang/playground`. `@fossil-lang/ui` does NOT own the FossilTheme
  shape — it only consumes the emitted CSS variables at runtime.
- License Apache-2.0 (matches the rest of `@fossil-lang/*`)
- `publishConfig.access = public` + `provenance = true`
- size budget 50 KB gzipped (per CONTEXT.md Phase 10 cap)
- `sideEffects` field set to the ARRAY form (per ADR-0034) so the
  style-injection module is retained by bundlers while everything else
  tree-shakes

Re-export from `@fossil-lang/playground` in Phase 10 plan 10-06
(`export * from '@fossil-lang/ui'` in playground's `index.tsx`) so existing
v0.1.x consumers can `import { Tabs, TabsList, ... } from
'@fossil-lang/playground'` for free.

## Consequences

**Positive:**

- Editor (Phase 11) and Viewer (Phase 12) can directly depend on
  `@fossil-lang/ui` without pulling the whole playground.
- `@fossil-lang/types` stays zero-runtime.
- DAG dependency direction — no cycles.

**Negative:**

- One additional package to publish + maintain (9 packages in v0.2.0).

**Neutral:**

- Naming: `@fossil-lang/ui` matches shadcn/Keasy convention so migrators
  recognise it.
