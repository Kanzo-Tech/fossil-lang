#!/usr/bin/env bash
# gen-types.sh — Generate src/generated.ts from the fossil-graph JSON Schemas.
#
# Single source of truth = the schemars-derived schemas in crates/fossil-graph
# (the same ones snapshot-tested in tests/schemas.rs). No hand-written TS types:
# this regenerates them from Rust on every `pnpm run build`.
#
#   cargo run --example dump_schemas   →   one combined JSON Schema doc
#   json2ts                            →   src/generated.ts
#
# Run via `pnpm --filter @fossil-lang/corpus gen:types` (wired in package.json).

set -euo pipefail

SCRIPT_DIR="$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
PKG_DIR="$( cd -- "$SCRIPT_DIR/.." &> /dev/null && pwd )"
REPO_ROOT="$( cd -- "$PKG_DIR/../.." &> /dev/null && pwd )"

OUT="$PKG_DIR/src/generated.ts"
TMP="$(mktemp -t fossil-graph-schemas.XXXXXX.json)"
trap 'rm -f "$TMP"' EXIT

echo "[gen-types] cargo run -p fossil-graph --example dump_schemas"
( cd "$REPO_ROOT" && cargo run --quiet -p fossil-graph --example dump_schemas ) > "$TMP"

echo "[gen-types] json2ts → $OUT"
# --additionalProperties false: structs use `deny_unknown_fields`; mirror that.
# The root `FossilGraphSchemas` interface is a harmless codegen by-product; the
# per-verb Params/Result interfaces are what the client consumes.
pnpm --filter @fossil-lang/corpus exec json2ts \
  --input "$TMP" \
  --output "$OUT" \
  --additionalProperties false \
  --bannerComment "/* eslint-disable */
/**
 * GENERATED — do not edit by hand.
 * Source: crates/fossil-graph JSON Schemas (schemars). Regenerate with
 *   pnpm --filter @fossil-lang/corpus gen:types
 */"

echo "[gen-types] done."
