//! DCAT-AP catalog graph builder — the catalog is "just another graph".
//!
//! Given the governance values + the run's dataset structure ([`CatalogInput`]),
//! this builds the DCAT-AP vertex + edge graph as literal rows and assembles it
//! through [`crate::literal::from_literal_graph`] — the SAME GraphAr output the
//! program path produces, with no executor and no DuckDB. The DCAT-AP *shape*
//! (which vertex types, which predicate IRIs) lives here in fossil (host
//! boundary), not re-implemented in keasy.
//!
//! Replaces the former `fossil-sinks::catalog` SQL-`VALUES` `SinkPlan` builder:
//! the data was always literal, so it never needed a query engine.

use fossil_graph_schema::{Cardinality, DataType as ScalarType, Property as NodeProp};
use fossil_run_status::CatalogInput;

use crate::literal::{from_literal_graph, LiteralEdge, LiteralVertex};
use crate::GraphArData;

// ── DCAT-AP vocabulary ───────────────────────────────────────────────────────
const DCAT_CATALOG: &str = "http://www.w3.org/ns/dcat#Catalog";
const DCAT_DATASET: &str = "http://www.w3.org/ns/dcat#Dataset";
const DCAT_DISTRIBUTION: &str = "http://www.w3.org/ns/dcat#Distribution";
const DCAT_ACCESS_URL: &str = "http://www.w3.org/ns/dcat#accessURL";
const DCAT_MEDIA_TYPE: &str = "http://www.w3.org/ns/dcat#mediaType";
const DCAT_KEYWORD: &str = "http://www.w3.org/ns/dcat#keyword";
const DCT_TITLE: &str = "http://purl.org/dc/terms/title";
const DCT_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
const DCT_ISSUED: &str = "http://purl.org/dc/terms/issued";
const DCT_LICENSE: &str = "http://purl.org/dc/terms/license";
const DCT_SOURCE: &str = "http://purl.org/dc/terms/source";
const DCT_CONFORMS_TO: &str = "http://purl.org/dc/terms/conformsTo";
const DCT_LANGUAGE: &str = "http://purl.org/dc/terms/language";
const FOAF_AGENT: &str = "http://xmlns.com/foaf/0.1/Agent";
const FOAF_NAME: &str = "http://xmlns.com/foaf/0.1/name";
const FOAF_HOMEPAGE: &str = "http://xmlns.com/foaf/0.1/homepage";
const VCARD_KIND: &str = "http://www.w3.org/2006/vcard/ns#Kind";
const VCARD_HAS_EMAIL: &str = "http://www.w3.org/2006/vcard/ns#hasEmail";
const KEASY_FIELD: &str = "urn:keasy:vocab#Field";

const EDGE_DATASET: &str = "dataset";
const EDGE_DISTRIBUTION: &str = "distribution";
const EDGE_FIELD: &str = "field";
const EDGE_PUBLISHER: &str = "publisher";
const EDGE_CONTACT_POINT: &str = "contactPoint";

/// Build the DCAT-AP catalog [`GraphArData`] from a host's [`CatalogInput`].
/// One row per catalog / dataset / distribution / field / agent / contact; the
/// cross-type references are GraphAr edges resolved against the URN subjects.
#[must_use]
#[allow(clippy::too_many_lines)] // a flat, linear DCAT-AP graph spec — clearer as one piece
pub fn build_catalog_graph(input: &CatalogInput) -> GraphArData {
    let mut vertices: Vec<LiteralVertex> = Vec::new();
    let mut edges: Vec<LiteralEdge> = Vec::new();

    let catalog_iri = catalog_urn(&input.job_id);
    let title = input
        .job_name
        .clone()
        .unwrap_or_else(|| "Keasy Pipeline Output".to_string());
    let lang = input.language.clone().unwrap_or_else(|| "en".to_string());

    vertices.push(LiteralVertex {
        label: "Catalog".into(),
        rdf_type: Some(DCAT_CATALOG.into()),
        properties: vec![
            prop("title", DCT_TITLE, ScalarType::String),
            prop("description", DCT_DESCRIPTION, ScalarType::String),
            prop("issued", DCT_ISSUED, ScalarType::DateTime),
            prop("language", DCT_LANGUAGE, ScalarType::String),
            prop("license", DCT_LICENSE, ScalarType::String),
        ],
        rows: vec![vec![
            catalog_iri.clone(),
            title,
            input.catalog_description.clone().unwrap_or_default(),
            input.completed_at.clone(),
            lang,
            input.license_uri.clone().unwrap_or_default(),
        ]],
    });

    if !input.datasets.is_empty() {
        vertices.push(LiteralVertex {
            label: "Dataset".into(),
            rdf_type: Some(DCAT_DATASET.into()),
            properties: vec![
                prop("title", DCT_TITLE, ScalarType::String),
                prop("source", DCT_SOURCE, ScalarType::String),
                prop("conforms_to", DCT_CONFORMS_TO, ScalarType::String),
                prop("keywords", DCAT_KEYWORD, ScalarType::String),
                data_col("entity_count", ScalarType::Integer),
            ],
            rows: input
                .datasets
                .iter()
                .map(|ds| {
                    vec![
                        dataset_urn(&input.job_id, &ds.type_name),
                        ds.type_name.clone(),
                        ds.source_name.clone().unwrap_or_default(),
                        ds.rdf_type.clone().unwrap_or_default(),
                        ds.keywords.join(", "),
                        ds.entity_count.unwrap_or(0).to_string(),
                    ]
                })
                .collect(),
        });
    }

    let dist_rows: Vec<Vec<String>> = input
        .datasets
        .iter()
        .flat_map(|ds| ds.distributions.iter())
        .map(|dist| {
            let filename = dist.destination.rsplit('/').next().unwrap_or(&dist.destination);
            vec![
                distribution_urn(&input.job_id, filename),
                dist.destination.clone(),
                dist.media_type.clone(),
            ]
        })
        .collect();
    if !dist_rows.is_empty() {
        vertices.push(LiteralVertex {
            label: "Distribution".into(),
            rdf_type: Some(DCAT_DISTRIBUTION.into()),
            properties: vec![
                prop("access_url", DCAT_ACCESS_URL, ScalarType::String),
                prop("media_type", DCAT_MEDIA_TYPE, ScalarType::String),
            ],
            rows: dist_rows,
        });
    }

    let agent_iri = input
        .publisher_uri
        .clone()
        .unwrap_or_else(|| publisher_urn(&input.publisher_name));
    vertices.push(LiteralVertex {
        label: "Agent".into(),
        rdf_type: Some(FOAF_AGENT.into()),
        properties: vec![
            prop("name", FOAF_NAME, ScalarType::String),
            prop("homepage", FOAF_HOMEPAGE, ScalarType::String),
        ],
        rows: vec![vec![
            agent_iri.clone(),
            input.publisher_name.clone(),
            input.publisher_uri.clone().unwrap_or_default(),
        ]],
    });

    let contact_iri = input.contact_email.as_ref().map(|email| {
        let iri = contact_urn(email);
        vertices.push(LiteralVertex {
            label: "Contact".into(),
            rdf_type: Some(VCARD_KIND.into()),
            properties: vec![prop("email", VCARD_HAS_EMAIL, ScalarType::String)],
            rows: vec![vec![iri.clone(), format!("mailto:{email}")]],
        });
        iri
    });

    let field_rows: Vec<Vec<String>> = input
        .datasets
        .iter()
        .flat_map(|ds| {
            ds.fields.iter().map(move |f| {
                vec![
                    field_urn(&input.job_id, &ds.type_name, &f.name),
                    f.name.clone(),
                    f.rdf_uri.clone().unwrap_or_default(),
                    f.datatype.clone().unwrap_or_else(|| "string".to_string()),
                ]
            })
        })
        .collect();
    if !field_rows.is_empty() {
        vertices.push(LiteralVertex {
            label: "Field".into(),
            rdf_type: Some(KEASY_FIELD.into()),
            properties: vec![
                data_col("name", ScalarType::String),
                data_col("rdf_uri", ScalarType::String),
                data_col("datatype", ScalarType::String),
            ],
            rows: field_rows,
        });
    }

    // ── Edges ────────────────────────────────────────────────────────────────
    push_edge(&mut edges, EDGE_DATASET, "Catalog", "Dataset", None, {
        input
            .datasets
            .iter()
            .map(|ds| (catalog_iri.clone(), dataset_urn(&input.job_id, &ds.type_name)))
            .collect()
    });
    push_edge(&mut edges, EDGE_DISTRIBUTION, "Dataset", "Distribution", None, {
        input
            .datasets
            .iter()
            .flat_map(|ds| {
                let src = dataset_urn(&input.job_id, &ds.type_name);
                ds.distributions.iter().map(move |dist| {
                    let filename =
                        dist.destination.rsplit('/').next().unwrap_or(&dist.destination);
                    (src.clone(), distribution_urn(&input.job_id, filename))
                })
            })
            .collect()
    });
    push_edge(&mut edges, EDGE_FIELD, "Dataset", "Field", None, {
        input
            .datasets
            .iter()
            .flat_map(|ds| {
                let src = dataset_urn(&input.job_id, &ds.type_name);
                ds.fields
                    .iter()
                    .map(move |f| (src.clone(), field_urn(&input.job_id, &ds.type_name, &f.name)))
            })
            .collect()
    });
    push_edge(
        &mut edges,
        EDGE_PUBLISHER,
        "Catalog",
        "Agent",
        None,
        vec![(catalog_iri.clone(), agent_iri)],
    );
    if let Some(contact_iri) = contact_iri {
        push_edge(
            &mut edges,
            EDGE_CONTACT_POINT,
            "Catalog",
            "Contact",
            None,
            vec![(catalog_iri, contact_iri)],
        );
    }

    from_literal_graph(vertices, edges)
}

/// A literal-object property carrying its RDF predicate IRI.
fn prop(name: &str, rdf_uri: &str, datatype: ScalarType) -> NodeProp {
    NodeProp {
        name: name.to_string(),
        datatype,
        iri: Some(rdf_uri.to_string()),
        cardinality: Cardinality::Single,
    }
}

/// A non-RDF data column (schema metadata — no predicate IRI).
fn data_col(name: &str, datatype: ScalarType) -> NodeProp {
    NodeProp {
        name: name.to_string(),
        datatype,
        iri: None,
        cardinality: Cardinality::Single,
    }
}

/// Push an edge type (skipping it when it has no endpoint pairs). A catalog has
/// many datasets/distributions per source vertex, so edges are multi-valued.
fn push_edge(
    edges: &mut Vec<LiteralEdge>,
    label: &str,
    src_type: &str,
    dst_type: &str,
    rdf_uri: Option<String>,
    pairs: Vec<(String, String)>,
) {
    if pairs.is_empty() {
        return;
    }
    edges.push(LiteralEdge {
        label: label.to_string(),
        rdf_uri,
        src_type: src_type.to_string(),
        dst_type: dst_type.to_string(),
        single_valued: false,
        pairs,
    });
}

// ── URN builders (ported verbatim from the DCAT materializer) ─────────────────
fn catalog_urn(job_id: &str) -> String {
    format!("urn:keasy:catalog:{job_id}")
}
fn dataset_urn(job_id: &str, type_name: &str) -> String {
    format!("urn:keasy:dataset:{job_id}/{}", encode_uri_component(type_name))
}
fn distribution_urn(job_id: &str, filename: &str) -> String {
    format!("urn:keasy:dist:{job_id}/{}", encode_uri_component(filename))
}
fn publisher_urn(name: &str) -> String {
    format!("urn:keasy:publisher:{}", slug(name))
}
fn contact_urn(email: &str) -> String {
    format!("urn:keasy:contact:{}", slug(email))
}
fn field_urn(job_id: &str, type_name: &str, field_name: &str) -> String {
    format!(
        "urn:keasy:field:{job_id}/{}/{}",
        encode_uri_component(type_name),
        encode_uri_component(field_name),
    )
}
fn slug(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '.', "-")
        .trim_matches('-')
        .to_string()
}
fn encode_uri_component(s: &str) -> String {
    s.replace(' ', "%20").replace('<', "%3C").replace('>', "%3E")
}
