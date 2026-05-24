# ADR 0031: Restructure repo as pnpm monorepo with `packages/` + `apps/`

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/research/playground-sources-design.md` §6 "Repo topology"
- ADR-0002 (15-crate Rust workspace layout — `crates/` is unchanged)
- ADR-0028 (React library distribution — six `@fossil-lang/*` packages need a home)
- ADR-0027, ADR-0029, ADR-0030 (the consumers of the new packages)

## Context

The repo today is a single-stack layout: `crates/` (Rust workspace, 15
crates), `playground/` (Vite TS app produced by Phase 7 plan 07-04),
`playground-poc/` (Phase 0 throwaway), `decisions/`, `.planning/`, and
the canonical design docs at the root.

ADR-0028's six-package family (`@fossil-lang/{wasm, types, codemirror-fossil,
resolvers, examples, playground}`) plus an `apps/landing/` Next.js host
does not fit the current layout. Options surveyed:

1. **Sibling repos** — each package its own repo. Rejected: six repos
   for a single coordinated release surface is operationally hostile;
   cross-package changes need synchronised PRs across repos; the
   compiler / packages version-sync becomes a CI nightmare.

2. **Single `playground/` directory that contains everything.**
   Rejected: blurs the line between published packages and demo
   host; no clear publish boundary; precludes Keasy importing
   just the codemirror extension.

3. **`packages/` + `apps/` workspace, single repo.** Accepted.
   Standard npm/Node monorepo layout; explicit publish boundary
   (every directory under `packages/` is published, every directory
   under `apps/` is not); fits alongside the existing `crates/`
   Rust workspace without conflict.

Tool choice: pnpm vs npm vs yarn. Keasy currently uses npm. pnpm has
faster installs, content-addressed store, strict dependency hoisting,
first-class workspace primitives, and the `pnpm-workspace.yaml`
declaration is unambiguous. The trade-off vs npm is a `pnpm` install
on contributor / CI machines. We accept this — pnpm is the dominant
monorepo workspace tool in the JS ecosystem (Vite itself, Vue, Astro,
Radix UI all use it).

The Rust workspace (`crates/`) is unchanged by this restructure.
`cargo` and `pnpm` coexist in the same repo without interference.

## Decision

Repo layout becomes:

```
rmlext/
├── crates/                       # Rust workspace (15 crates, unchanged)
│   ├── fossil-base/
│   ├── fossil-syntax/
│   ├── ...
│   └── fossil-wasm/              # ADR-0024 + ADR-0030 — exports Workspace API + tokenize
├── packages/                     # NEW — pnpm workspace, all published to npm
│   ├── wasm/                     # @fossil-lang/wasm — wraps fossil-wasm build outputs
│   ├── types/                    # @fossil-lang/types — shared TS types
│   ├── codemirror-fossil/        # @fossil-lang/codemirror-fossil — CM6 extension
│   ├── resolvers/                # @fossil-lang/resolvers — default/mock/public-HTTP
│   ├── examples/                 # @fossil-lang/examples — bundled fixture mappings
│   └── playground/               # @fossil-lang/playground — top-level React component
├── apps/                         # NEW — pnpm workspace, none published
│   └── landing/                  # Next.js minimal → playground.kanzo.dev
├── decisions/                    # ADRs (unchanged)
├── .planning/                    # GSD orchestration (gitignored)
├── pnpm-workspace.yaml           # NEW — declares packages/* + apps/*
├── package.json                  # NEW — root workspace package.json
├── Cargo.toml                    # Rust workspace (unchanged)
└── (canonical design docs + CLAUDE.md unchanged)
```

`playground-poc/` is deleted (was Phase 0 throwaway). The current
`playground/` directory (from Phase 7 plan 07-04) is deleted — its
contents are superseded by the new `packages/playground/` plus a
fresh skeleton appropriate to the React library design.

Tooling pins (proposed; revisit if any breaks):
- pnpm 9.x
- Node 20+ (LTS)
- TypeScript 5.6+
- Vite 7.x (library mode for packages; SSR for apps/landing)
- Next.js 15.x for the landing app
- Changesets for synchronised versioning across `@fossil-lang/*`

CI changes:
- Existing Rust jobs (`cargo check --workspace`, `cargo test`, WASM
  gate on 9 crates, `cargo deny`, fmt + clippy) — unchanged.
- New job: `pnpm install --frozen-lockfile && pnpm -r build && pnpm -r test`.
- New job: `apps/landing` build (Next.js + Playwright E2E).
- Bundle-size budget gate on `packages/playground/dist/`.

## Consequences

**Positive.**

- Clean publish boundary — every directory under `packages/` ships
  to npm; every directory under `apps/` doesn't.
- Workspace-local symlinks (pnpm) mean changes in `packages/wasm`
  immediately surface in `packages/codemirror-fossil` consumers
  during development — no local-publish dance.
- The Rust and JS workspaces coexist without interference. Contributors
  who only touch Rust never need pnpm; contributors who only touch
  the playground never need Cargo (beyond `cargo build --target
  wasm32-unknown-unknown -p fossil-wasm` to refresh the WASM bundle).
- Standard monorepo layout — onboarding cost is "this is a typical
  pnpm + cargo coexistence repo", not "this is a one-off structure".
- Changesets gives us synchronised version bumps across the six
  packages (a single PR can bump all of them coherently).

**Negative.**

- Contributors need `pnpm` installed alongside `cargo`. New tool to
  learn for Rust-first contributors. Mitigated by a `Makefile` /
  `task` wrapper that hides the distinction for common operations.
- CI matrix grows: existing Rust jobs + new pnpm jobs + new Next.js
  build + Playwright suite. CI time increases roughly proportionally.
  Mitigated by parallel job execution + pnpm's content-addressed
  cache (faster than npm by ~3×).
- `playground-poc/` and `playground/` deletion is a tracked-file
  deletion in the commit; reviewers must see and approve. Phase 0
  smoke / Phase 7 skeleton work is preserved in git history but no
  longer in the working tree.
- The README and CLAUDE.md "Project Layout" section need rewriting
  to document the dual-stack structure.

**Neutral.**

- The 9-crate WASM gate is unchanged (still gates `fossil-base`,
  `fossil-syntax`, `fossil-hir`, `fossil-mir`, `fossil-codegen`,
  `fossil-sinks`, `fossil-ide`, `fossil-ide-db`, `fossil-wasm`).
- All ADRs naming `crates/...` paths are unchanged.
- All `decisions/*.md` paths are unchanged.
- All `.planning/` content is unchanged in location.

## Alternatives considered

1. **Sibling repos (one per package).** Rejected — see Context.

2. **All-in-one `playground/` directory.** Rejected — no publish
   boundary; precludes piecemeal Keasy adoption.

3. **npm or yarn workspaces.** Rejected for pnpm because pnpm's
   workspace primitives are stricter, faster, and the dominant
   choice in adjacent ecosystems. Reversible at low cost if a Keasy
   alignment hard-requirement emerges (lockfile and workspace
   manifest change; package code unaffected).

4. **`apps/` as one of many `packages/` subdirectories with a
   private flag.** Rejected — explicit `apps/` is a stronger signal
   than `packages/landing` with `"private": true`. The directory
   name is the documentation.

5. **Defer the restructure to a separate later phase.** Rejected —
   ADR-0028 is unimplementable without the directory structure,
   and the restructure is the natural first plan of Phase 8.
