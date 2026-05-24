# ADR 0028: Distribute the playground as a React library (`@fossil-lang/*`)

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/research/playground-sources-design.md` §1, §5, §6
- `.planning/research/playground-sources-data-tools.md` (Observable / Hex / Sandpack distribution patterns)
- `.planning/research/playground-sources-rdf-tools.md` §"Embeddability"
- Keasy parts inventory (2026-05-24) — Keasy's `JobEditor` + `CodeEditor` + `PipelineFlow` + `GraphCanvas` are the existing duplicates this library is meant to replace
- ADR-0024 (`fossil-wasm` Workspace API — the Rust-side seam this library consumes)
- ADR-0027 (CodeMirror 6 — the editor inside the component)
- ADR-0029 (ConnectionResolver inversion-of-control — how the host provides credentials)
- ADR-0030 (WASM-exported tokenizer — how the grammar reaches the editor)
- ADR-0031 (pnpm monorepo restructure — physical home of the library)

## Context

The original Phase 7 plan modelled the playground as a standalone Vite app
deployed to `playground.kanzo.dev`. The 2026-05-24 redesign conversation
established three facts that make that model wrong:

1. **The playground has multiple hosts.** At minimum: the OSS landing,
   Keasy production, paper-demo / docs sites, and any third party that
   wants to embed a Fossil mapping editor with execution. A standalone
   app cannot be embedded; an iframe embed sacrifices integration and
   styling control.

2. **Keasy already implements partial duplicates** of every panel the
   playground exposes (script editor with `@`-autocomplete, pipeline
   DAG view, result graph). Maintaining two implementations would
   guarantee drift; consolidating into one library that Keasy consumes
   removes a chronic source of bugs.

3. **Reusability is a brand asset, not an afterthought.** Per the user's
   "diseño de referencia" framing, this library is expected to be the
   reference implementation of "browser-side typed mapping playground"
   for the KGC community — embed-grade quality, not demo-grade.

The component-shape options surveyed (Web Component, React library, vanilla
JS lib, iframe + postMessage) trade off integration depth vs. host
flexibility. React was chosen because (a) Keasy is Next.js, (b) the dominant
hosts in the target ecosystem (landing pages, docs, dashboards) are React-
era, and (c) React's component model maps cleanly to the playground's
panel composition. Non-React hosts pay the cost of bundling React; for the
target audience this is acceptable.

## Decision

The playground ships as a family of npm packages under the `@fossil-lang/`
scope:

- `@fossil-lang/playground` — top-level React component (`<FossilPlayground />`)
  composing editor + LSP + DuckDB-WASM runner + result viz; the default
  entry point for hosts that want the full experience.
- `@fossil-lang/codemirror-fossil` — CodeMirror 6 language extension
  (StreamParser + autocomplete + linter). Importable standalone for hosts
  that want only the editor (Keasy migration target).
- `@fossil-lang/wasm` — JS/TS wrapper around the `fossil-wasm` build
  artifacts (.js + .wasm + d.ts). Re-exports `FossilPlayground` class +
  `tokenize` + `semantic_legend` + Workspace API.
- `@fossil-lang/types` — shared TypeScript types: `SourceRef`,
  `ResolvedSource`, `Connector`, `ConnectionResolver`, `FossilTheme`,
  diagnostic shapes. Zero runtime cost.
- `@fossil-lang/resolvers` — default + mock + public-HTTP `ConnectionResolver`
  implementations. No host coupling.
- `@fossil-lang/examples` — bundled `.fossil/.csv/.csvw.json/.shex` example
  files. Reused by the landing app and embedded by `@fossil-lang/playground`
  as the default `examples` prop value.

Builds: Vite library mode → ESM + CJS + d.ts; React is `peerDependencies`
(host provides). The Rust workspace (15 crates in `crates/`) is unaffected.
A minimal Next.js landing app (`apps/landing/`) mounts `<FossilPlayground />`
with a public-only default resolver and deploys to `playground.kanzo.dev`.

Keasy migrates to consume `@fossil-lang/playground` (or `@fossil-lang/codemirror-fossil`
+ `@fossil-lang/wasm` à la carte if it wants finer control) and deprecates
its internal duplicates over a separate workstream outside Milestone 1.

## Consequences

**Positive.**

- Single source of truth for the playground UX across every host.
- Sub-package split enables hosts to import only what they need
  (Keasy might take just the CodeMirror extension + WASM bridge,
  not the full playground). Tree-shaking opportunity at consumer level.
- Versioning is sane: `@fossil-lang/playground@0.1.0` ↔ `fossil-wasm@0.1.0`
  ↔ compiler version, all published together.
- Reusability is the default; embedding is the test path. The component
  cannot accidentally grow host-coupled features (auth, persistence,
  AI) without crossing a published-API boundary first.
- Demo host (`apps/landing/`) is small and disposable; the "product"
  is the library, not the app.

**Negative.**

- React peer-dep locks out non-React hosts. Mitigated by the option of
  iframe-embed wrapping (each host can ship its own thin Next.js wrapper)
  but not eliminated.
- Six packages to maintain, version, document. Mitigated by single-repo
  monorepo (ADR-0031), Changesets for synchronised versioning, and
  shared tsconfig / build scripts.
- Public API surface freezes faster than an in-app design — every prop
  rename is a SemVer event. Mitigated by `0.x` versioning during
  Milestone 1 (explicit pre-stability signal).
- Bundle size is now a hard constraint, not a "later". Every dependency
  added to the playground is paid by every host. CI gates package size
  against budgets at publish time.

**Neutral.**

- The "standalone playground deployable to playground.kanzo.dev" exists
  as `apps/landing/` (Next.js minimal) and stays. The build artefact
  hosted at that URL is the landing app, but it is one consumer among
  many, not the product.
- Phase 9's VS Code extension is unaffected; it does not depend on
  the React library (it uses VS Code's Monaco directly).
- Keasy migration is a separate workstream — out of scope for Milestone 1,
  to be coordinated with the Keasy team once `@fossil-lang/playground`
  v0.1 publishes.

## Alternatives considered

1. **Web Component (custom element).** Framework-agnostic; embeddable
   anywhere. Rejected because the dominant hosts are React (Keasy is
   Next.js), the playground's panel composition maps poorly to Shadow
   DOM event/attribute communication, and React-inside-a-web-component
   doubles the boundary cost.

2. **Vanilla JS library (`mountPlayground(el, opts)`).** Framework-neutral
   imperative API. Rejected because every host then writes the same
   React/Vue/Svelte wrapper, the imperative state model fights React's
   declarative usage in the dominant host, and the API surface explodes
   to cover lifecycle (mount/unmount/dispose) that JSX gives for free.

3. **iframe + postMessage.** Maximum isolation, minimum host bundle.
   Rejected for primary distribution because iframe styling, sizing,
   keyboard-focus, and copy-paste integration are brittle for an editor-
   centric experience. iframe-embed remains available as a *wrapper
   pattern* hosts can apply on top of the React library if they need
   security isolation.

4. **Ship the playground as a Next.js app and tell Keasy to
   iframe-embed it.** Rejected — see (3) — and surrenders the Keasy
   migration to component-duplication forever.

5. **Single monolithic `@fossil-lang/playground` package without
   sub-package split.** Rejected because Keasy's natural migration path
   is "take the editor pieces first, then the runner, then the full
   playground"; without the split, that path collapses to "take it all
   or write your own", and Keasy would write its own.
