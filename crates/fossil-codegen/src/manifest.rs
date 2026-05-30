//! `GraphAr` manifest emission.
//!
//! Both compile paths render their manifest through ONE mechanism: the canonical
//! `fossil_sinks::manifest` structs serialised to `GraphAr` v1.0.0 YAML, joined
//! by [`concat_manifest`]. There is no hand-templated YAML — the manifest always
//! honestly describes the columns the SQL actually emits.
//!
//! - [`manifest_yaml_for_plan`] — the descriptor (`ShEx`) path: a decomposed
//!   [`SinkPlan`] → per-type vertex/edge manifests + the `GraphInfo` index, via
//!   the canonical W0b writer (`plan_manifests_from_sink_plan`). Identical to
//!   what `fossil run --dest` materialises.
//! - [`flat_triple_manifest`] — the schemaless (`AcceptAll` / walking-skeleton)
//!   path: the flat `(subject, predicate, object)` triple `output.parquet` has no
//!   shape to decompose against, so its honest manifest is a single `_triples`
//!   vertex type describing those three columns. Built from the same structs.

use std::fmt::Write as _;

use fossil_sinks::decomp::SinkPlan;
use fossil_sinks::manifest::{
    DEFAULT_CHUNK_SIZE, GraphInfo, Property, PropertyGroup, VertexInfo,
};
use fossil_sinks::writer::{WriteOptions, plan_manifests_from_sink_plan};

/// Join `(rel_path, yaml)` documents into one self-describing manifest file: a
/// `# rel_path` comment per document, `---` separators between them. The single
/// place this concatenation lives so the descriptor and schemaless paths share
/// byte-for-byte the same envelope.
fn concat_manifest<'a>(parts: impl Iterator<Item = (&'a str, &'a str)>) -> String {
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

/// Render the concatenated `GraphAr` v1.0.0 manifest YAML for a decomposed
/// [`SinkPlan`] (the descriptor / `ShEx` path — SINK-02).
///
/// Single source: delegates to the canonical W0b writer
/// ([`plan_manifests_from_sink_plan`]) — the same path `fossil run --dest`
/// materialises, so the `fossil compile` manifest carries the identical
/// `dense_id` + layout column shape and the top-level `GraphInfo` aggregate index.
#[must_use]
pub fn manifest_yaml_for_plan(plan: &SinkPlan) -> String {
    let set = plan_manifests_from_sink_plan(plan, &WriteOptions::default())
        .expect("a decomposed SinkPlan always yields a valid manifest");
    let parts = std::iter::once((set.graph.rel_path.as_str(), set.graph.yaml.as_str()))
        .chain(
            set.vertices
                .iter()
                .map(|m| (m.rel_path.as_str(), m.yaml.as_str())),
        )
        .chain(set.edges.iter().map(|m| (m.rel_path.as_str(), m.yaml.as_str())));
    concat_manifest(parts)
}

/// Render the honest `GraphAr` v1.0.0 manifest for the schemaless flat-triple
/// `output.parquet` (the `AcceptAll` / walking-skeleton path).
///
/// With no shape target there is nothing to decompose, so the output is a flat
/// `(subject, predicate, object)` triple table. The manifest describes exactly
/// those three columns as a single `_triples` vertex type — built from the same
/// canonical structs as the descriptor path (no hand-templated YAML), so it can
/// never claim columns the SQL does not emit.
#[must_use]
pub fn flat_triple_manifest() -> String {
    let triples = VertexInfo::new(
        "_triples",
        DEFAULT_CHUNK_SIZE,
        // The schemaless output is the single `output.parquet` at the dataset
        // root, not a per-type chunk directory.
        String::new(),
        vec![PropertyGroup {
            file_type: "parquet".to_string(),
            properties: vec![
                Property {
                    name: "subject".to_string(),
                    data_type: "string".to_string(),
                    is_primary: true,
                    is_nullable: Some(false),
                },
                Property {
                    name: "predicate".to_string(),
                    data_type: "string".to_string(),
                    is_primary: false,
                    is_nullable: Some(false),
                },
                Property {
                    name: "object".to_string(),
                    data_type: "string".to_string(),
                    is_primary: false,
                    is_nullable: None,
                },
            ],
        }],
    );
    let graph = GraphInfo::new(
        "graph",
        String::new(),
        vec!["_triples.vertex.yml".to_string()],
        Vec::new(),
    );
    let triples_yaml = triples.to_yaml().expect("VertexInfo serialises");
    let graph_yaml = graph.to_yaml().expect("GraphInfo serialises");
    concat_manifest(
        [
            ("graph.graph.yml", graph_yaml.as_str()),
            ("_triples.vertex.yml", triples_yaml.as_str()),
        ]
        .into_iter(),
    )
}
