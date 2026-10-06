//! Repository automation, as a library so its own tests can call it.
//!
//! `main.rs` is the `cargo xtask <command>` entrypoint; everything a command
//! actually does lives here, because a generator whose only caller is a binary
//! cannot be tested without spawning `cargo`.
//!
//! `depgraph` is not a command: it is the dependency graph that `wasm-check`
//! and the guards in `tests/` read.

pub mod catalogue;
pub mod corpus;
pub mod depgraph;
pub mod problem;
pub mod reference;
