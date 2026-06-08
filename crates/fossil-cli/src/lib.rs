//! `fossil-cli` is a binary-only crate; this `lib.rs` exists only to host the
//! native-only `compile_error!` cfg-tripwire so accidental WASM CI inclusion
//! fails at compile time rather than at link/runtime. The real CLI surface
//! lives in `src/main.rs`.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-cli is native-only (depends on fossil-engine which uses bundled DuckDB); \
     do not add it to the WASM CI gate"
);
