//! `fossil-sinks` — output sink trait + `GraphAr` stub.
//!
//! Phase 1 ships the [`Sink`] trait + a [`GraphArSink`] that returns a
//! hand-templated YAML manifest constant. The same constant lives in
//! `fossil-codegen::manifest::manifest_template` for Phase 1 — the redundancy
//! is intentional. Phase 5 (SINK-01..06) collapses both into this crate atop
//! `arrow` + `parquet` + `serde_yaml_ng` once programmatic manifest generation
//! + chunked COPY + `ShEx`-driven vertex/edge decomposition land.
//!
//! Phase 1's trait surface is deliberately minimal — `name()` + `manifest_template()`.
//! Phase 5 grows the trait with `vertex_edge_decomp(plan: &MirGraph) -> SinkPlan`,
//! `manifest(plan: &SinkPlan) -> Vec<u8>`, and `sql_for(plan: &SinkPlan) -> Vec<SqlStatement>`.
//! Additive-only: Phase 1's two methods stay.
//!
//! Phase 5 (SINK-02, ADR-0016) adds the [`manifest`] module: programmatic `GraphAr` v1.0.0
//! vertex-info/edge-info structs serialized via `serde_yaml_ng`, superseding the hand-templated
//! [`Sink::manifest_template`] (whose `graphar_version:`/`vertex_types:` spelling conformed to no
//! `GraphAr` reader). The codegen-side duplicate `manifest_template` is collapsed in plan 05-08.

pub mod manifest;

/// Output sink trait. Phase 1 surface is `name()` + `manifest_template()`.
/// Phase 5 SINK-01..06 adds programmatic decomposition + manifest + SQL emission.
pub trait Sink: Send + Sync + std::fmt::Debug {
    /// Stable, lowercase, namespace-free identifier (e.g. `"graphar"`).
    /// Used by the CLI to select sinks via `--sink graphar`.
    ///
    /// Trait signature returns `&str` (not `&'static str`) so Phase 5
    /// implementations can return dynamically-computed names (e.g. a
    /// parameterised `GraphArSink::with_namespace(ns)` whose name is
    /// stored in the struct).
    fn name(&self) -> &str;

    /// Phase 1: returns a hand-templated manifest constant.
    ///
    /// **Superseded (SINK-02, ADR-0016):** the Phase-1 spelling (`graphar_version: 1.0.0`,
    /// `vertex_types:`, `data_type: string`) conforms to no `GraphAr` reader. Programmatic
    /// generation now lives in [`crate::manifest`] ([`crate::manifest::VertexInfo`] /
    /// [`crate::manifest::EdgeInfo`] → `serde_yaml_ng`, `GraphAr` v1.0.0 field names). This method
    /// is retained additively per the Phase-1 trait contract and is collapsed with the
    /// codegen-side duplicate in plan 05-08.
    fn manifest_template(&self) -> String;
}

/// `GraphAr` sink (Apache `GraphAr` v1.0.0 manifest + Parquet vertex/edge chunks).
///
/// Phase 1 stub: emits a fixed YAML claiming a single `Person` vertex with a
/// `name` property — aspirational, since Phase 1 actually emits a flat triple
/// Parquet that does not yet conform to the `GraphAr` per-vertex chunk layout.
/// Phase 5 SINK-01..06 brings the emitted Parquet into compliance and replaces
/// `manifest_template()` with programmatic generation.
#[derive(Debug, Default)]
pub struct GraphArSink;

impl Sink for GraphArSink {
    // Phase 1 returns a literal; the trait signature stays `&str` so Phase 5
    // implementations can return dynamic strings (see Sink::name() doc).
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "graphar"
    }

    fn manifest_template(&self) -> String {
        // Trailing newline is intentional — keeps the YAML POSIX-clean and
        // matches `fossil_codegen::manifest::manifest_template` byte-for-byte
        // (the redundancy is by design; collapsed in Phase 5).
        "\
# GraphAr manifest — Phase 1 skeletal form.
graphar_version: 1.0.0
prefix: https://example.org/
vertex_types:
  - name: Person
    chunk_size: 1024
    properties:
      - name: name
        data_type: string
        nullable: false
    parquet_path: output.parquet
edge_types: []
"
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphar_sink_emits_valid_yaml_manifest_template() {
        let sink = GraphArSink;
        let m = sink.manifest_template();
        assert!(m.contains("graphar_version: 1.0.0"));
        assert!(m.contains("vertex_types:"));
        assert!(m.contains("Person"));
        assert!(m.contains("output.parquet"));
    }

    #[test]
    fn graphar_sink_name_is_stable() {
        let sink = GraphArSink;
        assert_eq!(sink.name(), "graphar");
    }
}
