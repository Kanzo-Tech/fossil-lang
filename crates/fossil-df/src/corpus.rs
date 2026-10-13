//! The `fossil/1` corpus format, which is the writer's: nothing else builds a
//! manifest.
//!
//! [`manifest`] is `fossil.json`: the structs are the format, [`crate::write()`]
//! builds and serialises them, and `fossil.schema.json` beside them is their
//! JSON Schema for a reader in another language. [`generated`] is the writer's
//! fixed columns, from `corpus.bnf`.

/// The writer's column table, generated from `corpus.bnf`.
pub mod generated;
pub mod manifest;
