//! `GraphAr` manifest emission.
//!
//! ## The descriptor (`ShEx`) path — programmatic `GraphAr` v1.0.0 (05-08, SINK-02)
//!
//! [`manifest_yaml_for_plan`] renders the real `GraphAr` v1.0.0 vertex-info /
//! edge-info YAML for a decomposed [`SinkPlan`] by delegating to
//! `fossil_sinks`'s `manifest_for_plan` (the 05-05 / 05-06 `VertexInfo` /
//! `EdgeInfo` structs, `version: gar/v1`). This is what the descriptor seam
//! ([`crate::sql::codegen_sql_with_descriptor`]) emits alongside the chunked
//! COPY SQL.
//!
//! ## The flat-triple (`AcceptAll` / walking-skeleton) path — Phase-1 template
//!
//! [`manifest_template`] is the Phase-1 hand-templated skeletal manifest used by
//! the descriptor-LESS flat-COPY path (`hello.fossil`). The duplicated constant
//! that previously lived inline here is collapsed: it now delegates to
//! `fossil_sinks`'s `GraphArSink::manifest_template` — one source of truth (05-08
//! manifest-dup collapse). The string is byte-identical, so the walking-skeleton
//! snapshot stays green. Superseded by the programmatic path for any real `ShEx`
//! target; retained because the `AcceptAll` demo emits a flat triple Parquet that
//! does not yet conform to the `GraphAr` per-vertex chunk layout.

use std::fmt::Write as _;

use fossil_sinks::Sink;
use fossil_sinks::decomp::SinkPlan;
use fossil_sinks::writer::{WriteOptions, plan_manifests_from_sink_plan};

/// Return the Phase-1 hand-templated `GraphAr` manifest for the flat-triple
/// (`AcceptAll` / walking-skeleton) path.
///
/// Collapsed (05-08): delegates to `fossil_sinks`'s `GraphArSink::manifest_template`
/// rather than re-declaring the constant, so the codegen-side and sink-side
/// copies can never drift. Byte-identical to the prior inline constant — the
/// `compile_hello` snapshot stays green.
#[must_use]
pub fn manifest_template() -> String {
    fossil_sinks::GraphArSink.manifest_template()
}

/// Render the concatenated `GraphAr` v1.0.0 manifest YAML for a decomposed
/// [`SinkPlan`] (the descriptor / `ShEx` path — SINK-02).
///
/// Single source: delegates to the canonical W0b writer
/// ([`plan_manifests_from_sink_plan`]) — the same path `fossil run --dest`
/// materialises, so the `fossil compile` manifest carries the identical
/// `dense_id` + layout column shape and the top-level `GraphInfo` aggregate
/// index. The per-file YAML documents (graph info, then each vertex/edge) are
/// concatenated with a `---` separator and a `# rel_path` comment so a single
/// emitted manifest file stays self-describing without a directory listing.
#[must_use]
pub fn manifest_yaml_for_plan(plan: &SinkPlan) -> String {
    let set = plan_manifests_from_sink_plan(plan, &WriteOptions::default())
        .expect("a decomposed SinkPlan always yields a valid manifest");
    let parts = std::iter::once((&set.graph.rel_path, &set.graph.yaml))
        .chain(set.vertices.iter().map(|m| (&m.rel_path, &m.yaml)))
        .chain(set.edges.iter().map(|m| (&m.rel_path, &m.yaml)));
    let mut out = String::new();
    for (rel_path, yaml) in parts {
        if !out.is_empty() {
            out.push_str("---\n");
        }
        writeln!(out, "# {rel_path}").expect("writing to a String never fails");
        out.push_str(yaml);
    }
    out
}
