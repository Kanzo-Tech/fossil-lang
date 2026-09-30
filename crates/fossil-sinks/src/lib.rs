//! `fossil-sinks` — the `fossil/1` corpus format.
//!
//! [`manifest`] is `fossil.json`: the structs are the format, `fossil-df`
//! builds and serialises them, and `fossil.schema.json` beside this crate is
//! their JSON Schema for a reader in another language. [`generated`] is the
//! writer's fixed columns, from `corpus.bnf`.
//!
//! It byte-writes nothing, and takes `arrow-schema` alone rather than the
//! `arrow` umbrella: that is what keeps it buildable for wasm32, where it sits
//! in the gate closure through `fossil-df-wasm`.

/// The writer's column table, generated from `corpus.bnf`.
pub mod generated;
pub mod manifest;
