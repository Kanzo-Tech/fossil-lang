#!/usr/bin/env bash
#
# bless-baselines.sh — Regenerate Playwright PNG baselines after a deliberate
# visual change. Run this from anywhere in the repo; it resolves the workspace
# root from its own dirname.
#
# # Why this script exists
#
# Per 17-CONTEXT.md: blessing baselines is local-script + commit-manual ONLY.
# No GitHub Action-triggered baseline regeneration. This script is the single
# mechanism humans use after deliberately changing playground visuals (theme
# tweak, layout adjustment, new feature in <FossilEditor/>, etc.).
#
# # Usage
#
#   pnpm bless-baselines                          # all visual specs in apps/landing
#   pnpm bless-baselines -- -g "Mapping tab"      # subset by test name pattern
#   pnpm bless-baselines -- tests/e2e/foo.spec.ts # specific spec file
#
# # Snapshot owner
#
# For v0.2 the only Playwright snapshot owner is `apps/landing/`. Future
# snapshot owners (other packages adding their own `__snapshots__` dirs)
# should extend this script with autodetection — see the workspace-package
# autodetection note in 17-03's SUMMARY.
#
# # Workflow after running
#
# 1. This script regenerates the PNGs under
#    `apps/landing/tests/visual/__snapshots__/`.
# 2. Inspect the diffs with `git diff` or a PNG viewer.
# 3. Commit the new baselines as part of the PR that introduced the visual
#    change: `git add apps/landing/tests/visual/__snapshots__/ && git commit`.

set -euo pipefail

# Resolve repo root from the script's own directory so this works
# regardless of the caller's cwd.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Ensure WASM artefact is built. Visual baselines depend on the playground
# rendering, which needs @fossil-lang/wasm's .wasm + .js outputs (Phase 15
# lesson — a stale WASM produces wrong tokenizer output → wrong syntax
# highlighting → spurious "diffs" that aren't real regressions).
pnpm --filter @fossil-lang/wasm build:wasm

# Forward all positional arguments to playwright. If no spec / pattern was
# provided, default to the Phase 15 + 17-04 visual-baselines spec.
ARGS=("$@")
if [ ${#ARGS[@]} -eq 0 ]; then
  ARGS=(tests/e2e/visual-baselines.spec.ts)
fi

pnpm --filter @fossil-lang/landing exec playwright test --update-snapshots "${ARGS[@]}"

echo
echo "✓ Baselines regenerated."
echo "  Review the diffs visually, then commit:"
echo "    git add apps/landing/tests/visual/__snapshots__/"
echo "    git commit -m 'test: bless visual baselines for <deliberate change>'"
