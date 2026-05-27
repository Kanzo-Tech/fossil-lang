# ADR 0038: Cross-repo consumption via npm pack + file: protocol (Phase 16 pre-publish smoke)

**Date:** 2026-05-27
**Status:** accepted
**Decider:** Angel Iglesias (Kanzo)
**Cite:** Phase 16 ROADMAP entry (`.planning/ROADMAP.md` — "Phase 16 keasy-migration"); Phase 17 REL-01 (`.planning/REQUIREMENTS.md` — release requirements); ADR-0031 (pnpm monorepo restructure)

## Context

Phase 16 migrates the Keasy product (`/Users/angel.ip/dev/kanzo/keasy/keasy/`) to
consume `@fossil-lang/*` packages BEFORE those packages are formally published to
the npm registry — that publish is a Phase 17 deliverable (REL-01). Without an
explicit cross-repo consumption mechanism, every downstream Phase 16 plan
(16-02 token bridge; 16-03 viewer swap; 16-04 editor swap + in-tree deletions)
is blocked: Keasy cannot `import { FossilEditor } from "@fossil-lang/editor"`
unless the package resolves to *some* on-disk artefact.

The two repos are sibling, independently versioned pnpm workspaces. Verified at
plan-time:

- `/Users/angel.ip/dev/kanzo/keasy/keasy/pnpm-workspace.yaml` — empty (`{}`)
- `/Users/angel.ip/dev/kanzo/keasy/rmlext/pnpm-workspace.yaml` — scoped to
  `packages/*` + `apps/*` of the Fossil repo only

There is NO shared parent pnpm workspace. ADR-0031 enshrines this independence
intentionally — the two products have distinct release cadences (Fossil is OSS
with Changesets-driven minor bumps; Keasy is closed-source SaaS with continuous
deploy) and a shared workspace would entangle their CI matrices, version locks,
and release tooling.

Three alternatives were evaluated:

**A. Pre-release npm publish under a `0.2.0-rc.0` dist-tag.** Would let Keasy
`pnpm add @fossil-lang/editor@rc` immediately. Wastes an irreversible
npm-version on un-validated code (npm only allows `unpublish` within 72h of
publish; after that the version is permanently burned). Risks Keasy consumers
discovering rc tarballs in search. Rejected.

**B. pnpm workspace overlay** — create a parent `pnpm-workspace.yaml` at
`/Users/angel.ip/dev/kanzo/keasy/` that includes both `keasy/` and `rmlext/`,
enabling `workspace:*` symlinks. Defeats ADR-0031 (the two repos become coupled
at the lockfile level); both repos' CIs would need to know about the parent;
GitHub Actions caching of `pnpm-store` would have to be reworked across two
repos. Rejected.

**C. `npm pack` tarball + `file:` protocol.** Keasy treats each `@fossil-lang/*`
package as a vendored dependency: the Fossil repo builds .tgz tarballs (via
`pnpm pack`, which honours the `files` allowlist in each package.json
identically to `npm publish`), drops them into a gitignored directory under
`keasy/web/vendor/fossil-lang/`, and Keasy's `package.json` references them via
`"@fossil-lang/editor": "file:./vendor/fossil-lang/fossil-lang-editor-0.1.0.tgz"`.
Verified: `npm pack --dry-run` and `pnpm pack` emit byte-identical archives to
what `npm publish` would ship (same `files` allowlist, same exports map, same
`sideEffects` flag, same `dependencies` block).

Decision: C.

## Decision

For Phase 16 only, Keasy consumes `@fossil-lang/*` via `pnpm pack`-generated
.tgz tarballs vendored under `keasy/web/vendor/fossil-lang/` and referenced
from `keasy/web/package.json` using the `file:` protocol. Phase 17 REL-01
supersedes this by publishing to the npm registry under the `latest` dist-tag;
the `file:` entries are flipped to `^0.2.0` (or whichever minor lands) at
Phase 17 close.

The rebuild loop is documented and one-shot:

```bash
cd /Users/angel.ip/dev/kanzo/keasy/rmlext
./scripts/pack-for-keasy.sh                        # builds + packs 8 packages
cd /Users/angel.ip/dev/kanzo/keasy/keasy/web
pnpm install                                       # resolves file: entries
pnpm build                                         # validates dep declaration
```

The pack script handles the eight `@fossil-lang/*` packages currently needed
(direct + transitive): `types`, `resolvers`, `codemirror-fossil`, `wasm`, `ui`,
`kanzo-theme` (published as `@kanzo/theme`), `viewer`, `editor`. Iteration
order is dependency-first so each `pnpm pack` resolves clean.

The drop-zone directory (`keasy/web/vendor/fossil-lang/`) is committed empty
via `.gitkeep` so the `file:` paths resolve before tarballs land. The .tgz
artefacts themselves are gitignored — local-only, regenerated on demand,
ephemeral.

## Consequences

**Positive:**

- Byte-identical to what npm would ship — `pnpm pack` uses the same `files`
  allowlist as `npm publish` would, so this is a real-world smoke test of the
  published-package shape (files allowlist, exports map, sideEffects flag,
  peerDependencies declaration) BEFORE the irreversible npm publish.
- Zero workspace coupling — the two repos remain independently releasable;
  ADR-0031 stays intact.
- Easy rollback — delete tarballs, revert the `file:` lines in Keasy's
  package.json, run `pnpm install` to drop the deps.
- Validates Phase 17 REL-01 prerequisites empirically: if `pnpm pack` produces
  a tarball that Keasy can't consume, we discover it now (Phase 16) rather
  than after the irreversible npm publish (Phase 17).
- No `npm unpublish` window pressure — tarballs are local artefacts, not
  registry-tracked versions.

**Negative:**

- Manual rebuild loop — changes to Fossil packages require re-running
  `scripts/pack-for-keasy.sh` + `pnpm install` in Keasy. Acceptable because
  Phase 16 is a one-shot migration, not an ongoing development loop; Phase 17
  ends the loop entirely (Keasy switches to registry deps).
- Tarballs are NOT committed (gitignored at the drop-zone level) — local-only
  artefacts. The `.gitkeep` keeps the directory shape so `pnpm install`
  doesn't choke on a non-existent path resolution.
- The hard-coded absolute path to Keasy in `scripts/pack-for-keasy.sh` is a
  project invariant — if the user reorganises the sibling-repo layout, they
  must update this ADR + the script together. A path env-var was considered
  and rejected: this is a one-shot script for a one-shot migration, the
  indirection costs more than the flexibility provides.

**Neutral:**

- The eight packages packed cover the full transitive closure Keasy needs
  even though Keasy's direct imports (per 16-03 + 16-04) are only `@fossil-lang/viewer`
  and `@fossil-lang/editor` — the remaining six come along as transitive
  deps. Bundling all eight in one pack invocation keeps the rebuild loop
  single-command.

## Compliance

Aligns with ADR-0031 (pnpm + cargo coexist; independent workspaces). Supersedes
nothing. Will be superseded by Phase 17 REL-01 (npm registry publish flips the
consumption mechanism from `file:` to `^x.y.z`).

## References

- Phase 16 ROADMAP entry — `.planning/ROADMAP.md`
- Phase 17 REL-01 — `.planning/REQUIREMENTS.md`
- ADR-0031 — pnpm monorepo restructure (`decisions/0031-pnpm-monorepo-restructure.md`)
- npm pack semantics — https://docs.npmjs.com/cli/v10/commands/npm-pack
- pnpm pack semantics — https://pnpm.io/cli/pack
- Phase 16 plan 16-01 — `.planning/phases/16-keasy-migration/16-01-PLAN.md`
