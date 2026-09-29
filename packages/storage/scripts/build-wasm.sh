#!/usr/bin/env bash
# build-wasm.sh — Build packages/storage/pkg/ from crates/fossil-storage-wasm/.
#
# Sibling of packages/wasm/scripts/build-wasm.sh (the same wasm-bindgen
# `--target web` recipe). The only differences are the
# crate (`fossil-storage-wasm`) and the artefact stem (`fossil_storage_wasm`).
#
# Pinned tooling (CLAUDE.md):
#   Rust toolchain: 1.90 (rust-toolchain.toml at repo root)
#   wasm-bindgen-cli: 0.2.120 (matches the workspace-pinned lib)
#
# Output: packages/storage/pkg/
#   fossil_storage_wasm.js          — JS shim (wasm-bindgen `--target web`)
#   fossil_storage_wasm.d.ts        — TS types for the JS shim
#   fossil_storage_wasm_bg.wasm     — the WASM binary
#   fossil_storage_wasm_bg.wasm.d.ts — TS types for the .wasm module

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

HAS_WASM_OPT=0
if command -v wasm-opt &> /dev/null; then
  HAS_WASM_OPT=1
else
  echo "::warning::wasm-opt not found. Install via brew (binaryen) or apt (binaryen). Skipping optimization pass — artefact will be larger." >&2
fi

# `ring` (object_store's signer) compiles C, so the wasm32 build needs a
# wasm-capable clang (Apple's lacks the target) — the executor's requirement too.
if [[ -z "${CC_wasm32_unknown_unknown:-}" && -x /opt/homebrew/opt/llvm/bin/clang ]]; then
  export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
  export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
fi

# ---- 1. Build the WASM via cargo ----

echo "[build-wasm] cargo build --profile wasm-release --target wasm32-unknown-unknown -p fossil-storage-wasm"
cargo build --profile wasm-release --target wasm32-unknown-unknown -p fossil-storage-wasm

WASM_INPUT="$REPO_ROOT/target/wasm32-unknown-unknown/wasm-release/fossil_storage_wasm.wasm"
if [[ ! -f "$WASM_INPUT" ]]; then
  echo "::error::cargo build did not produce expected artefact: $WASM_INPUT" >&2
  exit 1
fi

# ---- 2. Run wasm-bindgen --target web ----
#
# --target web (NOT bundler): the glue locates its .wasm with
# `new URL('<name>_bg.wasm', import.meta.url)`, which Vite, webpack 5 and
# Turbopack all emit as an asset — so a host calls init() with no argument and
# copies nothing. --target bundler instead emits `import * from '.wasm'` (ESM
# integration), which needs a per-bundler experiment flag. Nothing after this
# step may rewrite or inline the glue: tsc leaves pkg/ untouched.

PKG_DIR="$REPO_ROOT/packages/storage/pkg"
mkdir -p "$PKG_DIR"

echo "[build-wasm] wasm-bindgen --target web → $PKG_DIR"
wasm-bindgen "$WASM_INPUT" \
  --target web \
  --out-dir "$PKG_DIR"

# ---- 3. (Optional) wasm-opt -Oz for a smaller artefact ----
# The feature set is wasm32-unknown-unknown's default, as the executor's script
# explains; without it wasm-opt rejects the input.

if [[ $HAS_WASM_OPT -eq 1 ]]; then
  echo "[build-wasm] wasm-opt -Oz"
  wasm-opt -Oz \
    --enable-bulk-memory --enable-sign-ext --enable-mutable-globals \
    --enable-nontrapping-float-to-int --enable-reference-types --enable-multivalue \
    "$PKG_DIR/fossil_storage_wasm_bg.wasm" -o "$PKG_DIR/fossil_storage_wasm_bg.wasm"
fi

# ---- 4. Report size ----

RAW_SIZE=$(wc -c < "$PKG_DIR/fossil_storage_wasm_bg.wasm")
gzip -9 -k "$PKG_DIR/fossil_storage_wasm_bg.wasm"
GZ_SIZE=$(wc -c < "$PKG_DIR/fossil_storage_wasm_bg.wasm.gz")
rm "$PKG_DIR/fossil_storage_wasm_bg.wasm.gz"

echo "[build-wasm] fossil_storage_wasm_bg.wasm: $RAW_SIZE bytes raw / $GZ_SIZE bytes gzipped"
echo "[build-wasm] done. Artefacts in $PKG_DIR"
ls -lh "$PKG_DIR"
