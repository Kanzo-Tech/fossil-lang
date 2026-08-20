//! `fossil-lsp` — Language Server Protocol binary over `lsp-server` (stdio).
//!
//! This crate exists ONLY as a binary (see `src/main.rs`); the lib target
//! carries the `compile_error!` cfg-tripwire as a CI safety net. If a future
//! workspace-gate change accidentally ran `cargo check --target
//! wasm32-unknown-unknown -p fossil-lsp`, the lib-level tripwire would fail
//! fast — `main.rs` alone wouldn't catch lib-target compilation.
//!
//! The transport is `lsp-server`, not `tower-lsp`, and CLAUDE.md "Hard Rules"
//! (`fossil-lsp` is native-only). Mirrors the dual-tripwire layout of
//! `fossil-cli`: a binary-only crate still carries a `lib.rs`, and its only
//! content is the tripwire, so neither target can slip into the WASM gate.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-lsp is native-only (lsp-server uses crossbeam-channel + stdio); \
     the playground exposes LSP features via fossil-wasm directly, not through this binary"
);
