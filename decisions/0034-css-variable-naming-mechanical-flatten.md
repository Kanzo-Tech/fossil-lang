# ADR 0034: CSS variable naming convention — mechanical flatten of FossilTheme namespaces

**Date:** 2026-05-26
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** .planning/phases/10-visual-foundation-radix-ide-theme/10-CONTEXT.md (Revision 2026-05-26 — Reconciliation 1) + plan-checker iteration 1 findings.

## Context

The Fossil playground has shipped a mechanical CSS-variable flattener since
v0.1 Phase 8 plan 08-10 (`packages/playground/src/theme/tokens.ts`):

```ts
export function themeToCssVars(theme: FossilTheme): Record<string, string> {
  const vars: Record<string, string> = {};
  const walk = (obj: Record<string, unknown>, path: string[]): void => {
    for (const [k, v] of Object.entries(obj)) {
      const nextPath = [...path, k];
      if (typeof v === 'string') {
        vars[CSS_VAR_PREFIX + nextPath.join('-')] = v;
      } else if (v && typeof v === 'object') {
        walk(v as Record<string, unknown>, nextPath);
      }
    }
  };
  walk(theme as unknown as Record<string, unknown>, []);
  return vars;
}
```

The rule is intentionally mechanical and lossless: every leaf string at
object path `[a, b, c]` becomes the CSS custom property `--fossil-a-b-c`.

When Phase 10 adds five new IDE-grade token namespaces, the initially-locked
names in CONTEXT.md were singular kebab-case. However, the mechanical
flattener emits whatever shape the FossilTheme object has — if leaf keys are
camelCase the var names are camelCase. There were also two collisions:
`radius` and `space` namespaces already exist in FossilTheme (v0.1 legacy).

## Decision

The FossilTheme namespace shape is the source of CSS variable naming truth.
The following rules govern token additions:

1. **No camelCase leaves.** Multi-word var names use ONE of two equivalent
   shapes (author's choice per locality):
   - **Sub-object nesting** (preferred for genuine hierarchies):
     - `motion.duration.fast` → `--fossil-motion-duration-fast`
     - `motion.duration.base` → `--fossil-motion-duration-base`
     - `size.control.base` → `--fossil-size-control-base`
     - `size.control.sm` → `--fossil-size-control-sm`
     - `size.control.lg` → `--fossil-size-control-lg`
   - **Kebab-quoted string keys** (acceptable when nesting feels forced):
     - `focus: { ring: '...' }` → `--fossil-focus-ring` (single-word leaf,
       no kebab needed)
     - `focus: { 'ring-offset': '1px' }` → `--fossil-focus-ring-offset`
   - **FORBIDDEN**: camelCase leaves like `durationFast`, `controlSm`,
     `ringOffset`.

2. **Plural namespaces for new IDE-grade scales** to avoid collision with
   v0.1 legacy:
   - `radii` (new) alongside `radius` (v0.1 legacy, kept untouched)
   - `spacing` (new) alongside `space` (v0.1 legacy, kept untouched)
   - `focus` (new) — no v0.1 ancestor
   - `size` (new) — no v0.1 ancestor

3. **Numeric-string leaf keys** for scale namespaces (Tailwind-aligned):
   `spacing['0']`, `spacing['1']`, `spacing['2']`, `spacing['3']`,
   `spacing['4']`, `spacing['6']`, `spacing['8']` — emitting
   `--fossil-spacing-0` through `--fossil-spacing-8`. The `s` prefix
   (`s0`, `s1`) is forbidden — adds nothing and breaks Tailwind 1:1 mapping.

4. **Pre-resolved rgba values** for the focus ring expression (no
   `color-mix`). The `colors.ring` slot carries a hex value
   (default = `colors.accent`); the `focus.ring` token holds the FULL
   composed CSS expression as a pre-resolved rgba string (e.g.
   `0 0 0 3px rgba(59, 130, 246, 0.5)`). Rationale:
   `color-mix(in srgb, ...)` is unsupported in Safari < 16.2 (late 2022).
   Pre-resolving at theme-definition time costs ~zero (themes are static
   objects) and works in every browser supporting CSS custom properties
   (>99% global per caniuse).

5. **No abbreviations** in leaf keys: `sm`/`md`/`lg`/`xl`/`full` are accepted
   (industry-standard t-shirt sizes), but single-letter prefixes
   (`m`, `s`, `t`) and ad-hoc abbreviations (`prog`, `bg`) are forbidden.

6. **Legacy v0.1 namespaces are immutable.** `colors`, `fonts`, `space`,
   `radius` — these shipped in v0.1.x and their leaves cannot change. New
   IDE-grade namespaces are ADDED alongside; deprecation of legacy is a
   future major-bump concern (v1.0 earliest).

7. **The `size.control` token** (the canonical control-height, 32px) is
   NAMED `size.control.base` and emits `--fossil-size-control-base`. The
   unsuffixed `--fossil-size-control` is NOT emitted (the flattener cannot
   emit both a leaf AND children at the same path). Primitives reference
   `--fossil-size-control-base` for the default height. This is a small but
   deliberate naming choice: `base` makes the role explicit and gives
   `sm`/`lg` symmetric siblings.

## Consequences

**Positive:**

- CSS-var names are predictable from the FossilTheme shape — readers do not
  need to consult a separate mapping table.
- Adding new tokens is structurally enforced (TypeScript + the flattener)
  without touching the flattener.
- Pre-resolved rgba focus ring works in every browser supporting CSS custom
  properties (>99% global).
- v0.1.x consumers asserting `--fossil-radius-sm` continue to work — no
  regression.

**Negative:**

- Authors writing new tokens must internalise the rule. Mitigated by the
  ADR + an `@example` JSDoc on the FossilTheme interface in Plan 10-02.
- The `size.control.base` name (vs the original CONTEXT.md
  `--fossil-size-control`) is a small ergonomic loss — accepted to keep the
  flattener mechanical.

**Neutral:**

- The `colors.ring` slot defaults to the accent value; hosts who want a
  distinct focus ring colour override it explicitly.
- The `focus.ring` token IS the full box-shadow expression. Hosts overriding
  the entire ring set `--fossil-focus-ring` directly.

## Forbidden patterns

Listed for posterity:

- camelCase leaves (`durationFast`, `controlSm`, `focusRing`, `ringOffset`)
- `color-mix(in srgb, ...)` in token values
- Single-letter or abbreviated leaf keys (`m`, `s`, `t`, `prog`, `bg`)
- Mutating v0.1 legacy namespaces (`colors.*`, `fonts.*`, `space.*`,
  `radius.*`) — only ADD new sibling slots like `colors.ring`
- Adding token-naming logic to `themeToCssVars` — the flattener stays
  mechanical
