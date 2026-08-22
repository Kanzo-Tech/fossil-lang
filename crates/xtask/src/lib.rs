//! Repository automation, as a library so its own tests can call it.
//!
//! `main.rs` is the `cargo xtask <command>` entrypoint; everything a command
//! actually does lives here, because a generator whose only caller is a binary
//! cannot be tested without spawning `cargo`.

pub mod catalogue;
