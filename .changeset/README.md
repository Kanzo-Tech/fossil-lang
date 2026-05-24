# Changesets

This directory holds in-flight changesets that will be aggregated into the
next release of the `@fossil-lang/*` package family per ADR-0031 / ADR-0028.

## Workflow

1. Make code changes in a PR.
2. Run `pnpm changeset` (interactive) to capture a changeset describing
   the change (semver bump kind + summary).
3. Commit the generated `.changeset/*.md` file as part of the PR.
4. When the PR merges to `main`, the Changesets GitHub Action opens (or
   updates) a "Version Packages" PR that aggregates pending changesets
   and bumps versions in package.json files.
5. Merging the "Version Packages" PR triggers `pnpm release` which runs
   `pnpm -r build && changeset publish` with npm OIDC provenance.

## Linked packages

All `@fossil-lang/*` packages will bump in lockstep once they exist.
This avoids the "which version is compatible with which" matrix problem
for a six-package family with cross-dependencies.

The `linked` array in `config.json` is currently empty because
Changesets' validator rejects `linked` entries that don't match any
existing workspace member (and Wave 2-4 packages haven't landed yet).
Plan 08-12 (publish pipeline) will switch it to `[["@fossil-lang/*"]]`
once the six packages exist under `packages/*`.

## Why not Lerna / npm version?

Changesets captures bump intent at PR time (when context is fresh) and
aggregates at release time (when the changelog generation has full
information). See [Changesets docs](https://github.com/changesets/changesets).
