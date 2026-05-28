# Migrating to `@fossil-lang/* v0.2.0`

> Status: reference. v0.2.0 is the structural release — the `@fossil-lang/*` package family is complete (6 pre-existing packages + 2 new: `editor`, `viewer`). This guide is for adopters with a Keasy-like in-tree pattern (your own `<CodeEditor/>` + graph integration) who want to extract to the shared library. If you're new to Fossil, start at the playground: <https://playground.kanzo.dev> — you don't need this guide.

## Table of contents

1. [Quickstart](#quickstart)
2. [Migration recipe](#migration-recipe)
3. [Per-package consumption](#per-package-consumption)
   - [@fossil-lang/types](#fossil-langtypes)
   - [@fossil-lang/resolvers](#fossil-langresolvers)
   - [@fossil-lang/codemirror-fossil](#fossil-langcodemirror-fossil)
   - [@fossil-lang/wasm](#fossil-langwasm)
   - [@fossil-lang/examples](#fossil-langexamples)
   - [@fossil-lang/editor](#fossil-langeditor) (NEW in v0.2)
   - [@fossil-lang/viewer](#fossil-langviewer) (NEW in v0.2)
   - [@fossil-lang/playground](#fossil-langplayground)
4. [Transport decision tree](#transport-decision-tree)
5. [CSS theming integration](#css-theming-integration)
6. [Troubleshooting](#troubleshooting)
7. [Live demos + further reading](#live-demos--further-reading)

## Quickstart

<!-- ~30-line drop-in install recipe; existing v0.1.x → 0.2.0 + new adopters; peer deps; 3-line mount example. -->

## Migration recipe

<!-- Sequenced 5-step recipe addressed to a Keasy-like adopter. -->

### Step 1: Identify your in-tree components

<!-- grep + relative-import tighten; cite Phase 16 lesson. -->

### Step 2: Install `@fossil-lang/*` + plumb peer deps

<!-- pnpm add invocation + peer-dep matrix; CSS bridge forward-reference. -->

### Step 3: Replace your in-tree `<CodeEditor/>` with `<FossilEditor/>`

<!-- Literal Keasy diff from commit 1672fe6 + synthetic minimal. -->

### Step 4: Replace your in-tree graph with `<FossilViewer/>`

<!-- Literal Keasy diff from commit 7e8146c + synthetic minimal. -->

### Step 5: Delete in-tree files + reconcile relative imports

<!-- tsc-driven gotcha; the floating-controls/graph-settings story. -->

## Per-package consumption

### @fossil-lang/types

<!-- ~50 lines: purpose, exported types, synthetic usage. -->

### @fossil-lang/resolvers

<!-- ~50 lines: 3 impls + usage. -->

### @fossil-lang/codemirror-fossil

<!-- ~50 lines: fossil() composition, sub-extensions. -->

### @fossil-lang/wasm

<!-- ~50 lines: initFossilWasm + tokenize + lazy-load pattern (15-04). -->

### @fossil-lang/examples

<!-- ~50 lines: Example shape + the 6 bundled fixtures. -->

### @fossil-lang/editor

> **NEW in v0.2.** No v0.1.x baseline; this is the canonical extraction path for in-tree `<CodeEditor/>` components.

<!-- Section template + dedicated transport subsection (Http/Worker/Null) with prop signatures + literal Keasy snippet from 1672fe6. -->

### @fossil-lang/viewer

> **NEW in v0.2.** No v0.1.x baseline; this is the canonical extraction path for in-tree Cosmos.gl-based graph components.

<!-- Section template + dedicated CosmosGraphHandle API subsection (zoomIn/zoomOut/recenter — NOT raw zoom) + literal Keasy snippet from 7e8146c. -->

### @fossil-lang/playground

<!-- Composition root note. -->

## Transport decision tree

<!-- Text-only tree (no Mermaid) with Q1/Q2/Q3 + 10-LOC leaf examples. -->

## CSS theming integration

<!-- ~60 lines: --fossil-* tokens + Keasy-style alias bridge pattern + literal Keasy snippet from 16-02 globals.css. -->

## Troubleshooting

### Why was there a `pnpm.overrides` block in some early adopter setups?

<!-- ADR-0038 §Compliance cross-reference; literal block from 86056aa. -->

### My `<FossilViewer/>` floating-controls broke after upgrade

<!-- zoom → zoomIn/zoomOut; before/after from 7e8146c. -->

### `tsc` surfaced unexpected import errors after deletion

<!-- Sibling-import grep gotcha from Phase 16. -->

### `<FossilEditor/>` hover popup returns nothing

<!-- AnalysisHost.hover not yet wired upstream; degrades gracefully. -->

## Live demos + further reading

- Playground (5-tab demo, all features): <https://playground.kanzo.dev> (`apps/landing/`)
- Multi-host fixture (embedded-in-host-shell demo): <https://playground.kanzo.dev/multi-host> (`apps/landing/app/multi-host/`, NEW in v0.2 — built in plan 17-04)
- ADR-0038 (cross-repo consumption pattern): `decisions/0038-cross-repo-consumption.md`
- ADR-0034 (mechanical-flatten token vocabulary): `decisions/0034-css-variable-naming-mechanical-flatten.md`
- ADR-0035 (visual ownership separation): `decisions/0035-visual-ownership-separation.md`
- ADR-0036 (Transport-superset design): `decisions/0036-transport-superset.md`
- Keasy migration as a case study: Phase 16 plans 16-01..16-05 (see `.planning/phases/16-keasy-migration/16-SUMMARY.md`)
