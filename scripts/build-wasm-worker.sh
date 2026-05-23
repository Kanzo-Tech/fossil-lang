#!/usr/bin/env bash
# Build the fossil-wasm cdylib + wasm-bindgen JS glue for the playground
# LSP Worker. Outputs to playground/pkg-lsp-worker/ (--target web).
#
# Run from repo root: bash scripts/build-wasm-worker.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# 1. Pin guard — wasm-bindgen-cli MUST match the workspace wasm-bindgen
#    pin (=0.2.120 per CLAUDE.md / ADR-0014). Mismatching CLI fails
#    with a cryptic JS-glue contract error (Pitfall 2).
if ! command -v wasm-bindgen >/dev/null 2>&1; then
    echo "FAIL: wasm-bindgen-cli not installed."
    echo "      Run: cargo install wasm-bindgen-cli --version 0.2.120 --locked"
    exit 1
fi
WB_VER="$(wasm-bindgen --version | awk '{print $2}')"
if [[ "$WB_VER" != "0.2.120" ]]; then
    echo "FAIL: wasm-bindgen-cli is $WB_VER, expected 0.2.120"
    echo "      Run: cargo install wasm-bindgen-cli --version 0.2.120 --locked"
    exit 1
fi

# 2. Build the cdylib.
cargo build --target wasm32-unknown-unknown --release -p fossil-wasm

# 3. Run wasm-bindgen for --target web (Worker import target).
mkdir -p playground/pkg-lsp-worker
wasm-bindgen \
    target/wasm32-unknown-unknown/release/fossil_wasm.wasm \
    --out-dir playground/pkg-lsp-worker \
    --target web

echo "OK: playground/pkg-lsp-worker/ built"
