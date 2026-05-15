//! Fossil's WASM host shim.
//!
//! Phase 0: exposes a single `hello()` function returning a greeting.
//! Phase 1: replaced by FossilPlayground { check, compile, open_file, … }.

#![allow(unused)]

use wasm_bindgen::prelude::*;

/// Phase 0 smoke-test entry point. The 5-line index.html calls this
/// to prove the WASM toolchain end-to-end (Rust → wasm-bindgen → JS → DOM).
#[wasm_bindgen]
#[must_use]
pub fn hello() -> String {
    init_panic_hook();
    format!("hello from fossil-wasm v{}", env!("CARGO_PKG_VERSION"))
}

/// Idempotent panic-hook + console-log init. Called from every public entry point.
fn init_panic_hook() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}
