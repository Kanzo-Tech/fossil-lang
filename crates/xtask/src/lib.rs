//! Repository automation, as a library so its own tests can call it.
//!
//! `main.rs` is the `cargo xtask <command>` entrypoint; everything a command
//! actually does lives here, because a generator whose only caller is a binary
//! cannot be tested without spawning `cargo`.
//!
//! `depgraph` and `rulebook` are not commands: they are the two things the
//! repo-wide guards in `tests/` read — the dependency graph and `CLAUDE.md` —
//! and an integration test is its own crate, so a helper shared by two of them
//! has nowhere else to live.

pub mod catalogue;
pub mod corpus;
pub mod depgraph;
pub mod reference;
pub mod rulebook;
