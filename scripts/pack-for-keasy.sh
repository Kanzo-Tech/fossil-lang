#!/usr/bin/env bash
# pack-for-keasy.sh — Build + pack the @fossil-lang/* family as .tgz tarballs
# into Keasy's vendored drop-zone.
#
# Per ADR-0038 (decisions/0038-cross-repo-consumption.md): Phase 16 consumes
# Fossil packages cross-repo via `pnpm pack` + the `file:` protocol, BEFORE
# the formal npm-registry publish in Phase 17 REL-01.
#
# Usage:
#   ./scripts/pack-for-keasy.sh
#
# Output: 10 .tgz files appear in
#   /Users/angel.ip/dev/kanzo/keasy/keasy/web/vendor/fossil-lang/
#
# Then the operator runs (manually — this script does NOT touch Keasy):
#   cd /Users/angel.ip/dev/kanzo/keasy/keasy/web && pnpm install
#
# If you reorganise the sibling-repo layout, update BOTH this script's
# KEASY_VENDOR path AND ADR-0038 — they are intentionally coupled (the path
# is a project invariant, not an env-var override).

set -euo pipefail

FOSSIL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEASY_VENDOR="/Users/angel.ip/dev/kanzo/keasy/keasy/web/vendor/fossil-lang"

if [ ! -d "$KEASY_VENDOR" ]; then
  echo "ERROR: $KEASY_VENDOR not found." >&2
  echo "       Did 16-01 Task 3 create the drop-zone?" >&2
  echo "       Run: mkdir -p $KEASY_VENDOR && touch $KEASY_VENDOR/.gitkeep" >&2
  exit 1
fi

# Pack in dependency order so each pnpm pack resolves its in-workspace deps
# cleanly (transitive first). Ten packages, matching ADR-0038's enumeration:
PACKAGES=(
  types               # zero deps — leaf
  introspect          # zero @fossil-lang deps — leaf (schema introspection)
  resolvers           # depends on types
  codemirror-fossil   # depends on types + wasm (tokenize)
  wasm                # depends on types
  graph               # zero @fossil-lang runtime deps — bundles fossil-graph-wasm pkg/
  ui                  # zero @fossil-lang deps (Radix + theme tokens)
  kanzo-theme         # depends on types (published as @kanzo/theme)
  viewer              # depends on ui + types
  editor              # depends on codemirror-fossil + ui + types + wasm + resolvers
)

echo "Packing @fossil-lang/* family into $KEASY_VENDOR"
echo "FOSSIL_ROOT=$FOSSIL_ROOT"
echo ""

for pkg in "${PACKAGES[@]}"; do
  PKG_DIR="$FOSSIL_ROOT/packages/$pkg"
  if [ ! -d "$PKG_DIR" ]; then
    echo "ERROR: $PKG_DIR not found — package missing from workspace?" >&2
    exit 1
  fi
  echo "→ pnpm pack $pkg"
  cd "$PKG_DIR"
  pnpm pack --pack-destination "$KEASY_VENDOR"
done

echo ""
echo "Packed ${#PACKAGES[@]} tarballs into $KEASY_VENDOR"
ls -1 "$KEASY_VENDOR"/*.tgz
