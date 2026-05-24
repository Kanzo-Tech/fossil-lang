#!/usr/bin/env bash
# build-wasm.sh — Build packages/wasm/pkg/ from crates/fossil-wasm/ per ADR-0030 + RESEARCH.md Pattern 3.
#
# Pinned tooling (CLAUDE.md):
#   Rust toolchain: 1.90 (from rust-toolchain.toml at repo root)
#   wasm-bindgen-cli: 0.2.120 (matches lib version pinned in workspace Cargo.toml)
#
# Output: packages/wasm/pkg/
#   fossil_wasm.js          — JS shim for the WASM module (wasm-bindgen `--target web`)
#   fossil_wasm.d.ts        — TS types for the JS shim
#   fossil_wasm_bg.wasm     — the WASM binary itself
#   fossil_wasm_bg.wasm.d.ts — TS types for the .wasm module (wasm-bindgen 0.2.120 emits this)

set -euo pipefail

SCRIPT_DIR="$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
REPO_ROOT="$( cd -- "$SCRIPT_DIR/../../.." &> /dev/null && pwd )"

cd "$REPO_ROOT"

# ---- Preflight: tool availability ----

if ! command -v cargo &> /dev/null; then
  echo "::error::cargo not found. Install Rust toolchain (rustup) from https://rustup.rs/" >&2
  exit 1
fi

if ! command -v wasm-bindgen &> /dev/null; then
  echo "::error::wasm-bindgen CLI not found." >&2
  echo "Install pinned version 0.2.120 (matches Cargo.toml wasm-bindgen lib pin):" >&2
  echo "  cargo install --version 0.2.120 wasm-bindgen-cli" >&2
  exit 1
fi

WB_VERSION=$(wasm-bindgen --version | awk '{print $2}')
if [[ "$WB_VERSION" != "0.2.120" ]]; then
  echo "::warning::wasm-bindgen CLI version is $WB_VERSION; expected 0.2.120 to match CLAUDE.md pin. Build may produce ABI-mismatched output." >&2
fi

# wasm-opt is optional but recommended (smaller artefact for OFFLINE-01 caching).
HAS_WASM_OPT=0
if command -v wasm-opt &> /dev/null; then
  HAS_WASM_OPT=1
else
  echo "::warning::wasm-opt not found. Install via brew (binaryen) or apt (binaryen). Skipping optimization pass — artefact will be larger." >&2
fi

# ---- 1. Build the WASM via cargo ----

echo "[build-wasm] cargo build --release --target wasm32-unknown-unknown -p fossil-wasm"
cargo build --release --target wasm32-unknown-unknown -p fossil-wasm

WASM_INPUT="$REPO_ROOT/target/wasm32-unknown-unknown/release/fossil_wasm.wasm"
if [[ ! -f "$WASM_INPUT" ]]; then
  echo "::error::cargo build did not produce expected artefact: $WASM_INPUT" >&2
  exit 1
fi

# ---- 2. Run wasm-bindgen --target web ----
#
# Why --target web (NOT --target bundler): per RESEARCH.md Pitfall 1, the
# --target bundler output assumes the consumer's bundler handles .wasm ESM
# imports — fragile when republished as a library (consumer's Vite/Next/Webpack
# config may not). --target web produces a small JS shim where the consumer
# passes the resolved .wasm URL via init({ wasmUrl }) — explicit, predictable
# behaviour across every bundler model.

PKG_DIR="$REPO_ROOT/packages/wasm/pkg"
mkdir -p "$PKG_DIR"

echo "[build-wasm] wasm-bindgen --target web → $PKG_DIR"
wasm-bindgen "$WASM_INPUT" \
  --target web \
  --out-dir "$PKG_DIR"

# ---- 3. (Optional) Run wasm-opt -O3 for smaller artefact ----

if [[ $HAS_WASM_OPT -eq 1 ]]; then
  echo "[build-wasm] wasm-opt -O3"
  wasm-opt -O3 "$PKG_DIR/fossil_wasm_bg.wasm" -o "$PKG_DIR/fossil_wasm_bg.wasm"
fi

# ---- 4. Report size + assert PKG-03 (2 MB compressed) ----

RAW_SIZE=$(wc -c < "$PKG_DIR/fossil_wasm_bg.wasm")
gzip -9 -k "$PKG_DIR/fossil_wasm_bg.wasm"
GZ_SIZE=$(wc -c < "$PKG_DIR/fossil_wasm_bg.wasm.gz")
rm "$PKG_DIR/fossil_wasm_bg.wasm.gz"

PKG03_LIMIT=$((2 * 1024 * 1024))
echo "[build-wasm] fossil_wasm_bg.wasm: $RAW_SIZE bytes raw / $GZ_SIZE bytes gzipped (PKG-03 limit: $PKG03_LIMIT)"

if [[ "$GZ_SIZE" -gt "$PKG03_LIMIT" ]]; then
  echo "::error::WASM bundle exceeds 2 MB compressed (PKG-03): $GZ_SIZE > $PKG03_LIMIT bytes." >&2
  echo "Either profile + slim the fossil-wasm crate, or split into fossil-wasm-lex-only + fossil-wasm-full per ADR-0030 deferred item." >&2
  exit 1
fi

echo "[build-wasm] done. Artefacts in $PKG_DIR"
ls -lh "$PKG_DIR"
