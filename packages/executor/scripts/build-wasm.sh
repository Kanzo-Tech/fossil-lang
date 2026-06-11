#!/usr/bin/env bash
# build-wasm.sh — Build packages/executor/pkg/ from crates/fossil-df-wasm/.
#
# The DataFusion executor (datafusion + arrow + parquet-rs). SEPARATE artefact
# from @fossil-lang/wasm (the LSP shim) so it lazy-loads only when running a job.
#
# Pinned tooling: Rust 1.90 (rust-toolchain.toml), wasm-bindgen-cli 0.2.120.
#
# zstd-C: datafusion 54 hardcodes arrow-ipc["zstd"], so the wasm32 build needs a
# wasm-capable clang (Apple's lacks the target). Set CC_wasm32_unknown_unknown /
# AR_wasm32_unknown_unknown to an LLVM (brew install llvm). CI must install LLVM.
#
# Output: packages/executor/pkg/
#   fossil_df_wasm.js / .d.ts        — JS shim + types (wasm-bindgen --target web)
#   fossil_df_wasm_bg.wasm / .d.ts   — the WASM binary + its types

set -euo pipefail

SCRIPT_DIR="$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
REPO_ROOT="$( cd -- "$SCRIPT_DIR/../../.." &> /dev/null && pwd )"
cd "$REPO_ROOT"

if ! command -v cargo &> /dev/null; then
  echo "::error::cargo not found. Install Rust (rustup) from https://rustup.rs/" >&2
  exit 1
fi
if ! command -v wasm-bindgen &> /dev/null; then
  echo "::error::wasm-bindgen CLI not found. Install: cargo install --version 0.2.120 wasm-bindgen-cli" >&2
  exit 1
fi
WB_VERSION=$(wasm-bindgen --version | awk '{print $2}')
if [[ "$WB_VERSION" != "0.2.120" ]]; then
  echo "::warning::wasm-bindgen CLI version is $WB_VERSION; expected 0.2.120 (CLAUDE.md pin)." >&2
fi

# Default the zstd cross-clang to a brew LLVM if the caller didn't set it.
if [[ -z "${CC_wasm32_unknown_unknown:-}" && -x /opt/homebrew/opt/llvm/bin/clang ]]; then
  export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
  export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
fi

# --profile wasm-release (NOT plain --release): opt-level="z" + fat LTO + strip +
# panic=abort — the size-tuned profile for the lazy-loaded browser artefact.
echo "[build-wasm] cargo build --profile wasm-release --target wasm32-unknown-unknown -p fossil-df-wasm"
cargo build --profile wasm-release --target wasm32-unknown-unknown -p fossil-df-wasm

WASM_INPUT="$REPO_ROOT/target/wasm32-unknown-unknown/wasm-release/fossil_df_wasm.wasm"
if [[ ! -f "$WASM_INPUT" ]]; then
  echo "::error::cargo build did not produce $WASM_INPUT" >&2
  exit 1
fi

# --target web (NOT bundler): the consumer passes the resolved .wasm URL via
# initFossilExecutor({ wasmUrl }); predictable across every bundler (Pitfall 1).
PKG_DIR="$REPO_ROOT/packages/executor/pkg"
mkdir -p "$PKG_DIR"
echo "[build-wasm] wasm-bindgen --target web → $PKG_DIR"
wasm-bindgen "$WASM_INPUT" --target web --out-dir "$PKG_DIR"

# wasm-opt -Oz shrinks the (large, datafusion-heavy) artefact (~24MB → ~21MB raw
# / ~6MB gz). The feature set must be EXPLICIT, not `-all`:
#   - too few → wasm-opt aborts validating the input ("memory.copy operations
#     require bulk memory operations [--enable-bulk-memory-opt]");
#   - `-all` → wasm-opt is free to EMIT gc / typed-function-references in its
#     output (`(ref <heaptype>)`), which Node 20 / older browsers reject at
#     instantiation ("Invalid type '(ref <heaptype>)'").
# So enable exactly what wasm-bindgen 0.2.120 emits — the broadly-supported
# baseline — and nothing from the gc/function-references family. Needs binaryen
# ≥116 (the bulk-memory/bulk-memory-opt split); CI installs it in release.yml.
WASM_OPT_FEATURES=(
  --enable-bulk-memory --enable-bulk-memory-opt --enable-sign-ext
  --enable-mutable-globals --enable-nontrapping-float-to-int
  --enable-reference-types --enable-multivalue
)
if command -v wasm-opt &> /dev/null; then
  echo "[build-wasm] wasm-opt -Oz ${WASM_OPT_FEATURES[*]}"
  wasm-opt -Oz "${WASM_OPT_FEATURES[@]}" "$PKG_DIR/fossil_df_wasm_bg.wasm" -o "$PKG_DIR/fossil_df_wasm_bg.wasm"
else
  echo "::warning::wasm-opt not found (brew install binaryen). Skipping size pass — the executor artefact will be large (lazy-loaded, so acceptable but heavier)." >&2
fi

RAW_SIZE=$(wc -c < "$PKG_DIR/fossil_df_wasm_bg.wasm")
GZ_SIZE=$(gzip -9 -c "$PKG_DIR/fossil_df_wasm_bg.wasm" | wc -c)
echo "[build-wasm] fossil_df_wasm_bg.wasm: $RAW_SIZE bytes raw / $GZ_SIZE bytes gzipped"
echo "[build-wasm] done. Artefacts in $PKG_DIR"
