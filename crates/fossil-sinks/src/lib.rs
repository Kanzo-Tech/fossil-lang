//! `fossil-sinks` — output sink trait + `GraphAr` decomposition & manifest.
//!
//! The [`Sink`] trait selects an output strategy (`name()` +
//! [`Sink::vertex_edge_decomp`]); [`GraphArSink`] implements the Apache
//! `GraphAr` v1.0.0 strategy. Manifest emission is programmatic only — the
//! [`manifest`] module's `VertexInfo` / `EdgeInfo` / `GraphInfo` structs
//! serialised via `serde_yaml_ng` (ADR-0016), built once by
//! [`writer::plan_manifests`] and rendered by `fossil_codegen::manifest`. There
//! is no hand-templated manifest constant.
//!
//! - [`decomp`] — `ShEx`-driven vertex/edge decomposition into a [`SinkPlan`].
//! - [`writer`] — the W0b chunked-COPY SQL plan + `GraphInfo`-indexed manifest set.

pub mod catalog;
pub mod decomp;
pub mod graph_schema;
pub mod manifest;
pub mod writer;

use decomp::{SinkPlan, vertex_edge_decomp};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_mir::MirGraph;

/// Output sink trait. Surface is `name()` + `vertex_edge_decomp()`.
pub trait Sink: Send + Sync + std::fmt::Debug {
    /// Stable, lowercase, namespace-free identifier (e.g. `"graphar"`).
    /// Used by the CLI to select sinks via `--sink graphar`.
    ///
    /// Trait signature returns `&str` (not `&'static str`) so Phase 5
    /// implementations can return dynamically-computed names (e.g. a
    /// parameterised `GraphArSink::with_namespace(ns)` whose name is
    /// stored in the struct).
    fn name(&self) -> &str;

    /// Phase 5 (SINK-01/04/05, ADR-0018): decompose a mapping `plan` into a [`SinkPlan`] under a
    /// target shape descriptor.
    ///
    /// The `kind` descriptor is passed **directly** (SC#4 option (b), NOT read via `Db::system()`).
    /// The default delegates to the free function [`decomp::vertex_edge_decomp`]; sinks that need
    /// a bespoke decomposition (none in v0.1) may override. Additive — Phase-1 methods stay.
    fn vertex_edge_decomp<'db>(
        &self,
        plan: &MirGraph<'db>,
        db: &'db dyn fossil_base::Db,
        kind: &OutputDescriptorKind,
        chunk_size: u64,
    ) -> SinkPlan {
        vertex_edge_decomp(plan, db, kind, chunk_size)
    }
}

/// `GraphAr` sink (Apache `GraphAr` v1.0.0 manifest + Parquet vertex/edge chunks).
///
/// Selects the `GraphAr` decomposition strategy ([`Sink::vertex_edge_decomp`]);
/// manifest emission is the canonical `fossil_sinks::manifest` structs, rendered
/// by `fossil_codegen::manifest`.
#[derive(Debug, Default)]
pub struct GraphArSink;

impl Sink for GraphArSink {
    // The trait signature stays `&str` so a future parameterised sink can return
    // a dynamic name (see Sink::name() doc).
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "graphar"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphar_sink_name_is_stable() {
        let sink = GraphArSink;
        assert_eq!(sink.name(), "graphar");
    }
}
