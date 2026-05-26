# ADR 0035: Separate visual ownership — brand visuals owned by `@kanzo/*`; `@fossil-lang/ui` consumes the `--fossil-*` CSS-var contract

**Date:** 2026-05-26
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/10-visual-foundation-radix-ide-theme/10-09-PLAN.md` — surfaced mid-Phase-10 (after plan 10-06's `fossilIdeTheme` default-flip) as an architectural framing issue; corrected in plan 10-09.

## Context

v0.2 of the Fossil family is **WASM-first**. The compiler, LSP server-side,
playground runtime, type checker, and tokenizer all run client-side in
WebAssembly. The bulk of the v0.2 value — type-checked mappings with
incremental feedback in the browser, no backend round-trip — is host-agnostic
and brand-agnostic by construction.

The **visual layer** is the only remaining acoplamiento point. Phase 10 made
three coupled investments:

1. `@fossil-lang/ui` (ADR-0033) — Radix primitives wrapped behind the
   `@fossil-lang/*` namespace + the `cx()` utility + the static stylesheet
   injection for state pseudo-selectors. Each primitive references the
   `--fossil-*` CSS custom property contract at runtime.
2. THEME-01 + Phase 10 plan 10-02 extended the `FossilTheme` shape (in
   `@fossil-lang/types`) with IDE-grade namespaces (`radii`, `spacing`,
   `motion`, `focus`, `size`) per ADR-0034's mechanical-flatten convention.
3. Plan 10-06 added `packages/playground/src/theme/fossil-ide.ts` carrying
   `fossilIdeTheme` (lightTheme + 13px typography), flipped `useTheme`'s
   default + `<FossilPlayground/>`'s prop-destructure default to
   `'fossil-ide'`, and re-exported the theme value from
   `@fossil-lang/playground`.

The plan 10-06 invesment **locked the kanzo brand surface into the OSS
`@fossil-lang/playground` package**. Any consumer of the playground —
including non-kanzo hosts (research demos, fork forks, third-party
deployments) — would visually inherit the kanzo brand by default, with no
opt-out short of passing an explicit `theme` prop to override. The OSS
brand and the `kanzo.tech` commercial brand were entangled.

Three options surface for the correction:

1. **Status quo (do nothing)** — leave `fossilIdeTheme` inside
   `@fossil-lang/playground`. Documentation-only mitigation: rename the
   constant to something brand-neutral (`ideTheme`) and document the visual
   choice as "an opinionated IDE look the OSS Fossil family ships by
   default". Cheap. Coupling persists.
2. **Extract `@kanzo/theme` — brand-owned theme in its own package; revert
   `@fossil-lang/*` defaults.** OSS primitives become theme-less; hosts
   wrap their tree in `<KanzoThemeProvider/>` for the kanzo look.
3. **Shared `@kanzo-fossil/theme` package** — a joint brand+OSS package
   sitting between the two namespaces. Bridges both worlds. Adds a third
   namespace (`@kanzo-fossil/*`) the project doesn't otherwise need.

Bundle cost: `@kanzo/theme` is intentionally tiny (~5 KB realistic; 20 KB
cap) — a `FossilTheme` object + a ~30-line React Context Provider + a
vendored ~25-line mechanical-flatten helper. No Radix or DOM-heavy code.

Dep graph cost: `@kanzo/theme` depends on `@fossil-lang/types` (for the
`FossilTheme` type contract) and has a `peerDependencies` on React. It
does NOT depend on `@fossil-lang/playground` or `@fossil-lang/ui` — cycle
prevention by construction. `@fossil-lang/ui` continues to NOT depend on
`@kanzo/theme` — the OSS primitive surface stays brand-free.

## Decision

We adopt **option 2**. The visual ownership is separated:

- **Brand visuals are owned by the `@kanzo/*` family.** The v0.2 entry is
  `@kanzo/theme` (this ADR's deliverable, landed in plan 10-09). Future
  `@kanzo/icons`, `@kanzo/logos`, `@kanzo/composer-defaults` etc. may
  follow as the kanzo brand matures.
- **`@fossil-lang/ui` is theme-less.** The Radix primitive wrappers and
  the static stylesheet under `packages/ui/src/styles/inject.ts` reference
  `--fossil-*` CSS custom properties at runtime. With no theme provider
  supplying those vars, the primitives still wire ARIA + keyboard + state
  selectors correctly; only the visual chrome falls back to browser
  defaults.
- **`@fossil-lang/playground` v0.2 default behaviour:** when no `theme`
  prop is passed AND no ancestor provider supplies the cascade, the
  component renders against browser defaults. v0.1.x consumers explicitly
  passing `theme='light'` or `theme='dark'` continue to see IDENTICAL
  behaviour to v0.1.x. The `'fossil-ide'` string alias is REMOVED (it
  briefly existed in 10-06; it is no longer a built-in @fossil-lang/* theme
  name).
- **Kanzo-branded hosts** (apps/landing, multi-host-fixture primitives
  entry, future Keasy migration consumers) wrap their tree in
  `<KanzoThemeProvider/>` from `@kanzo/theme`. The Provider applies the
  brand cascade via a single wrapping `<div data-kanzo-theme/>` with the
  flattened `--fossil-*` vars on its `style` prop.
- **The `FossilTheme` type contract** (in `@fossil-lang/types`) is the
  ONLY shared shape between the namespaces. It remains owned by the OSS
  surface — brand-owned packages consume it.
- **The CSS-var contract** (`--fossil-*` per ADR-0034) is the runtime
  bridge. Both OSS primitives and brand-owned themes agree on the
  variable names + their semantic meaning via the FossilTheme shape.

## Consequences

**Positive:**

- `@fossil-lang/ui` bundle ships zero brand bytes. Non-kanzo consumers
  pay nothing for the kanzo brand.
- Non-kanzo hosts ship their own theme (a `FossilTheme` value + their own
  Provider, or just CSS-var overrides on a root element) without needing
  to opt OUT of a brand they never wanted.
- Cycle-free dep graph: `@kanzo/theme → @fossil-lang/types`;
  `@fossil-lang/ui` does not depend on `@kanzo/theme`. Future brand
  divergence (Keasy ships its own variants, third-party brands ship
  forks) does not need to touch the OSS surface.
- The v0.2 vision — "OSS toolchain runs in the browser; brand-owners
  layer their visual identity on top" — is now architecturally enforced,
  not just documented.

**Negative:**

- Kanzo-branded hosts MUST install `@kanzo/theme` and wrap their tree in
  `<KanzoThemeProvider/>`. The "free upgrade for v0.1.x consumers" of
  plan 10-06 is rescinded — non-kanzo v0.1.x consumers who never wanted
  the IDE look would have been silently upgraded by 10-06's default-flip;
  with this ADR they continue to see lightTheme behaviour (no upgrade,
  no surprise).
- One additional workspace package to publish + maintain (`@kanzo/theme`).
  Realistic maintenance burden: very low (text tokens evolve at the
  cadence of the kanzo brand, not the OSS Fossil cadence).

**Neutral:**

- The `UseThemeResult.theme` field's type widens from `FossilTheme` to
  `FossilTheme | undefined` to encode the "no theme provided" case. A
  workspace-wide grep confirms no in-workspace consumer accesses this
  field directly; v0.1.x external consumers only reached the value via
  `<FossilPlayground/>`'s prop boundary which is preserved.
- Future hosts that want neither kanzo nor the OSS defaults ship their
  own Provider. The Provider pattern (single wrapping div + cssVarsToStyle
  on its `style` prop) is documented in `packages/kanzo-theme/README.md`
  as the canonical template; any host can copy ~30 lines + their own
  FossilTheme to bootstrap a brand.

## Alternatives rejected

- **Option 1 (status quo)** — entanglement persists; the "WASM-first =
  host-agnostic" architectural narrative is undermined by one specific
  brand riding for free on the OSS bundle.
- **Option 3 (shared `@kanzo-fossil/theme`)** — adds a third namespace
  without resolving the underlying coupling. If kanzo+fossil ship a
  joint package, that package IS the OSS+brand bridge — same problem,
  different repo organisation. Rejected.
- **CSS-in-JS runtime (styled-components, Linaria, vanilla-extract)** —
  explicit non-goal per Phase 10 CONTEXT.md. CSS-var driven static styles
  is the canonical approach.

## Related ADRs

- [ADR-0033](0033-fossil-lang-ui-package.md) — establishes
  `@fossil-lang/ui` as the OSS primitive package. Plan 10-09 amends this
  ADR to record that the primitives are theme-less and the
  `fossil-ide default theme` originally added in 10-06 was relocated
  here.
- [ADR-0034](0034-css-variable-naming-mechanical-flatten.md) — establishes
  the `--fossil-*` CSS-var naming convention. Unchanged by this ADR;
  `@kanzo/theme`'s vendored `themeToCssVars` helper is governed by it.
- [ADR-0028](0028-playground-as-react-library.md) — the React-library
  shape of `@fossil-lang/playground`. The v0.2.x default behaviour
  (`<FossilPlayground/>` without a theme prop renders against browser
  defaults) is consistent with that ADR; brand-owners layer on top.
