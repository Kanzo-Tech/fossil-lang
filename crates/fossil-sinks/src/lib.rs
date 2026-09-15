//! `fossil-sinks` — the canonical `GraphAr` manifest model.
//!
//! The [`manifest`] module's `VertexInfo` / `EdgeInfo` / `GraphInfo` structs are
//! the single source of the `GraphAr` v1.0.0 layout, serialised via `serde_yaml_ng`.
//! The producer (`fossil-df`) and the reader (`fossil-graph`) build and
//! round-trip these structs — there is no second manifest shape.
//!
//! `arrow-schema` alone and never the `arrow` umbrella (see `Cargo.toml`) is
//! what keeps this buildable for wasm32; it is in the gate closure through
//! `fossil-df-wasm`.

/// The writer's column table, generated from `corpus.bnf`.
///
/// Here rather than in either half of the write path because this crate is the
/// junta: the manifest model is in the closure of both the writer and the
/// reader, so every Rust consumer of the vocabulary already depends on it and
/// none grows an edge to reach the table.
pub mod generated;
pub mod manifest;
