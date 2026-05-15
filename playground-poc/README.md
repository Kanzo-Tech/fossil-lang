# playground-poc — Phase 0 WASM smoke test

This directory is a **throwaway** Phase 0 artifact that proves the WASM
toolchain end-to-end. The real playground (Vite project) lands in Phase 7
under `playground/`. Delete or replace `playground-poc/` when Phase 7 begins.

## What this proves

Phase 0 success criterion #3 (per `.planning/ROADMAP.md`):
> A 5-line `index.html` loads the `fossil-wasm` shim and prints a string
> returned from a stub function.

## Prerequisites

- Rust toolchain 1.90 (per `rust-toolchain.toml`) with `wasm32-unknown-unknown` target.
- `wasm-bindgen-cli` version `=0.2.120` (must match the `wasm-bindgen` crate pin
  in workspace `Cargo.toml` exactly):
  ```bash
  cargo install wasm-bindgen-cli --version 0.2.120 --locked
  ```

## Build & run

From the repository root:

```bash
# 1. Build the WASM binary (release profile for size).
cargo build --target wasm32-unknown-unknown --release -p fossil-wasm

# 2. Generate JS glue into playground-poc/pkg/.
wasm-bindgen target/wasm32-unknown-unknown/release/fossil_wasm.wasm \
  --out-dir playground-poc/pkg \
  --target web

# 3. Serve the directory (CORS prevents file:// from working).
python3 -m http.server 8000

# 4. Open in browser.
open http://localhost:8000/playground-poc/
```

You should see the page render: **`hello from fossil-wasm v0.1.0`**

## Optional: size optimization

```bash
wasm-opt -O3 playground-poc/pkg/fossil_wasm_bg.wasm \
  -o playground-poc/pkg/fossil_wasm_bg.wasm
```

(Requires `binaryen`. Install: `brew install binaryen` on macOS.)

## Notes

- `playground-poc/pkg/` is gitignored. Each developer regenerates it locally.
- This directory exists only to prove the toolchain; the Phase 7 playground
  is a Vite project at `playground/` (lands later in the roadmap).
