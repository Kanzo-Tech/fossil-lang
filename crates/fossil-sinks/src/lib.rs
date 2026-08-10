//! `fossil-sinks` — the canonical `GraphAr` manifest model.
//!
//! The [`manifest`] module's `VertexInfo` / `EdgeInfo` / `GraphInfo` structs are
//! the single source of the `GraphAr` v1.0.0 layout, serialised via `serde_yaml_ng`
//! (ADR-0016). Both the producer (`fossil-df`, which materialises the `GraphAr`
//! tree) and the consumer (`fossil-graph`, the reader) build/round-trip these
//! structs — there is no hand-templated manifest constant and no parallel reader
//! shape.
//!
//! WASM-clean: depends on `arrow-schema` + `serde_yaml_ng` only (no arrow-ipc /
//! mio leak), so it joins the wasm gate.

pub mod manifest;
