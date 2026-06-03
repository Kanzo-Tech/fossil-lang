//! DCAT-AP catalog `SinkPlan` builder.
//!
//! The catalog is "just another output graph": given the governance VALUES + the
//! run's dataset structure ([`CatalogInput`]), this builds the DCAT-AP vertex +
//! edge [`SinkPlan`] that the SAME W0b writer materialises to `GraphAr` Parquet —
//! no Polars, no second materialiser in the host. The DCAT-AP *shape* (which
//! vertex types, which predicate IRIs) therefore lives in fossil (host
//! boundary), not re-implemented in keasy.
//!
//! Source relations are inline `(VALUES …) AS t(col, …)` — the catalog is small
//! (one row per dataset / distribution / field), so it needs no `CREATE VIEW`
//! prelude. Subjects are URNs in the `urn:keasy:*` namespace; literal columns
//! carry their DCAT/DCT/FOAF/vCard predicate IRI + XSD datatype so the catalog's
//! own manifest is self-describing (mirrors the run path's output spec).

use fossil_run_status::CatalogInput;

use crate::decomp::{EdgeTable, IRI_COLUMN, SinkPlan, VertexProperty, VertexTable};
use crate::manifest::DEFAULT_CHUNK_SIZE;

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
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

// Edge type labels (the GraphAr edge directory `edge/<src>_<label>_<dst>`).
const EDGE_DATASET: &str = "dataset";
const EDGE_DISTRIBUTION: &str = "distribution";
const EDGE_FIELD: &str = "field";
const EDGE_PUBLISHER: &str = "publisher";
const EDGE_CONTACT_POINT: &str = "contactPoint";

/// Build the DCAT-AP catalog `(prelude, SinkPlan)` from a host's [`CatalogInput`].
///
/// The prelude is always empty (sources are inline `VALUES`). The returned plan
/// feeds `writer::plan_writes_from_sink_plan` + `runtime::materialize_graph_ar`
/// exactly like a run, so the catalog is written by the one `GraphAr` writer.
#[must_use]
#[allow(clippy::too_many_lines)] // a flat, linear DCAT-AP graph spec — clearer as one piece
pub fn build_catalog_sink_plan(input: &CatalogInput) -> (String, SinkPlan) {
    let mut vertices = Vec::new();
    let mut edges = Vec::new();

    let catalog_iri = catalog_urn(&input.job_id);
    let title = input
        .job_name
        .clone()
        .unwrap_or_else(|| "Keasy Pipeline Output".to_string());
    let lang = input.language.clone().unwrap_or_else(|| "en".to_string());

    // ── Catalog (1) ──────────────────────────────────────────────────────────
    vertices.push(vertex(
        "Catalog",
        DCAT_CATALOG,
        &[
            prop("title", DCT_TITLE, XSD_STRING),
            prop("description", DCT_DESCRIPTION, XSD_STRING),
            prop("issued", DCT_ISSUED, XSD_DATETIME),
            prop("language", DCT_LANGUAGE, XSD_STRING),
            prop("license", DCT_LICENSE, XSD_STRING),
        ],
        &[vec![
            lit(&catalog_iri),
            lit(&title),
            lit(input.catalog_description.as_deref().unwrap_or("")),
            lit(&input.completed_at),
            lit(&lang),
            lit(input.license_uri.as_deref().unwrap_or("")),
        ]],
    ));

    // ── Dataset (N) ──────────────────────────────────────────────────────────
    if !input.datasets.is_empty() {
        let rows: Vec<Vec<String>> = input
            .datasets
            .iter()
            .map(|ds| {
                vec![
                    lit(&dataset_urn(&input.job_id, &ds.type_name)),
                    lit(&ds.type_name),
                    lit(ds.source_name.as_deref().unwrap_or("")),
                    lit(ds.rdf_type.as_deref().unwrap_or("")),
                    lit(&ds.keywords.join(", ")),
                    ds.entity_count.unwrap_or(0).to_string(),
                ]
            })
            .collect();
        vertices.push(vertex(
            "Dataset",
            DCAT_DATASET,
            &[
                prop("title", DCT_TITLE, XSD_STRING),
                prop("source", DCT_SOURCE, XSD_STRING),
                prop("conforms_to", DCT_CONFORMS_TO, XSD_STRING),
                prop("keywords", DCAT_KEYWORD, XSD_STRING),
                // entity_count is a numeric data column, not an RDF property.
                data_col("entity_count", "int64"),
            ],
            &rows,
        ));
    }

    // ── Distribution (M) ─────────────────────────────────────────────────────
    let dist_rows: Vec<Vec<String>> = input
        .datasets
        .iter()
        .flat_map(|ds| ds.distributions.iter())
        .map(|dist| {
            let filename = dist
                .destination
                .rsplit('/')
                .next()
                .unwrap_or(&dist.destination);
            vec![
                lit(&distribution_urn(&input.job_id, filename)),
                lit(&dist.destination),
                lit(&dist.media_type),
            ]
        })
        .collect();
    if !dist_rows.is_empty() {
        vertices.push(vertex(
            "Distribution",
            DCAT_DISTRIBUTION,
            &[
                prop("access_url", DCAT_ACCESS_URL, XSD_STRING),
                prop("media_type", DCAT_MEDIA_TYPE, XSD_STRING),
            ],
            &dist_rows,
        ));
    }

    // ── Agent (1) ────────────────────────────────────────────────────────────
    let agent_iri = input
        .publisher_uri
        .clone()
        .unwrap_or_else(|| publisher_urn(&input.publisher_name));
    vertices.push(vertex(
        "Agent",
        FOAF_AGENT,
        &[
            prop("name", FOAF_NAME, XSD_STRING),
            prop("homepage", FOAF_HOMEPAGE, XSD_STRING),
        ],
        &[vec![
            lit(&agent_iri),
            lit(&input.publisher_name),
            lit(input.publisher_uri.as_deref().unwrap_or("")),
        ]],
    ));

    // ── Contact (0/1) ────────────────────────────────────────────────────────
    let contact_iri = input.contact_email.as_ref().map(|email| {
        let iri = contact_urn(email);
        vertices.push(vertex(
            "Contact",
            VCARD_KIND,
            &[prop("email", VCARD_HAS_EMAIL, XSD_STRING)],
            &[vec![lit(&iri), lit(&format!("mailto:{email}"))]],
        ));
        iri
    });

    // ── Field (K) — schema metadata, no RDF predicate on its columns ─────────
    let field_rows: Vec<Vec<String>> = input
        .datasets
        .iter()
        .flat_map(|ds| {
            ds.fields.iter().map(move |f| {
                vec![
                    lit(&field_urn(&input.job_id, &ds.type_name, &f.name)),
                    lit(&f.name),
                    lit(f.rdf_uri.as_deref().unwrap_or("")),
                    lit(f.datatype.as_deref().unwrap_or("string")),
                ]
            })
        })
        .collect();
    if !field_rows.is_empty() {
        vertices.push(vertex(
            "Field",
            KEASY_FIELD,
            &[
                data_col("name", "string"),
                data_col("rdf_uri", "string"),
                data_col("datatype", "string"),
            ],
            &field_rows,
        ));
    }

    // ── Edges ────────────────────────────────────────────────────────────────
    // Catalog → Dataset
    if let Some(e) = edge(
        EDGE_DATASET,
        "Catalog",
        "Dataset",
        input
            .datasets
            .iter()
            .map(|ds| {
                (
                    catalog_iri.clone(),
                    dataset_urn(&input.job_id, &ds.type_name),
                )
            })
            .collect(),
    ) {
        edges.push(e);
    }
    // Dataset → Distribution
    if let Some(e) = edge(
        EDGE_DISTRIBUTION,
        "Dataset",
        "Distribution",
        input
            .datasets
            .iter()
            .flat_map(|ds| {
                let src = dataset_urn(&input.job_id, &ds.type_name);
                ds.distributions.iter().map(move |dist| {
                    let filename = dist
                        .destination
                        .rsplit('/')
                        .next()
                        .unwrap_or(&dist.destination);
                    (src.clone(), distribution_urn(&input.job_id, filename))
                })
            })
            .collect(),
    ) {
        edges.push(e);
    }
    // Dataset → Field
    if let Some(e) = edge(
        EDGE_FIELD,
        "Dataset",
        "Field",
        input
            .datasets
            .iter()
            .flat_map(|ds| {
                let src = dataset_urn(&input.job_id, &ds.type_name);
                ds.fields.iter().map(move |f| {
                    (
                        src.clone(),
                        field_urn(&input.job_id, &ds.type_name, &f.name),
                    )
                })
            })
            .collect(),
    ) {
        edges.push(e);
    }
    // Catalog → Agent
    if let Some(e) = edge(
        EDGE_PUBLISHER,
        "Catalog",
        "Agent",
        vec![(catalog_iri.clone(), agent_iri)],
    ) {
        edges.push(e);
    }
    // Catalog → Contact
    if let Some(contact_iri) = contact_iri
        && let Some(e) = edge(
            EDGE_CONTACT_POINT,
            "Catalog",
            "Contact",
            vec![(catalog_iri, contact_iri)],
        )
    {
        edges.push(e);
    }

    (
        String::new(),
        SinkPlan {
            vertices,
            edges,
            chunk_size: DEFAULT_CHUNK_SIZE,
        },
    )
}

/// A literal-object property column (carries its RDF predicate + XSD datatype).
fn prop(name: &'static str, rdf_uri: &'static str, xsd: &'static str) -> VertexProperty {
    VertexProperty {
        name: name.to_string(),
        data_type: "string".to_string(),
        rdf_uri: Some(rdf_uri.to_string()),
        xsd_datatype: Some(xsd.to_string()),
        single_valued: true,
    }
}

/// A non-RDF data column (schema metadata — no predicate / datatype IRI).
fn data_col(name: &'static str, graphar: &'static str) -> VertexProperty {
    VertexProperty {
        name: name.to_string(),
        data_type: graphar.to_string(),
        rdf_uri: None,
        xsd_datatype: None,
        single_valued: true,
    }
}

/// Build one [`VertexTable`] from its properties + already-formatted VALUES rows
/// (each row is `[iri_literal, prop0_literal, …]`).
fn vertex(
    type_name: &str,
    rdf_type: &str,
    props: &[VertexProperty],
    rows: &[Vec<String>],
) -> VertexTable {
    let mut columns = vec![IRI_COLUMN.to_string()];
    columns.extend(props.iter().map(|p| p.name.clone()));
    let alias = format!("cat_{}", type_name.to_lowercase());
    VertexTable {
        type_name: type_name.to_string(),
        rdf_type: Some(rdf_type.to_string()),
        vertex_id_col: IRI_COLUMN.to_string(),
        properties: props.to_vec(),
        source_relation: values_relation(&alias, &columns, rows),
    }
}

/// Build one [`EdgeTable`] from `(src_iri, dst_iri)` pairs, or `None` when empty.
fn edge(
    label: &str,
    src_type: &str,
    dst_type: &str,
    pairs: Vec<(String, String)>,
) -> Option<EdgeTable> {
    if pairs.is_empty() {
        return None;
    }
    let rows: Vec<Vec<String>> = pairs
        .into_iter()
        .map(|(s, d)| vec![lit(&s), lit(&d)])
        .collect();
    Some(EdgeTable {
        src_type: src_type.to_string(),
        predicate: label.to_string(),
        dst_type: dst_type.to_string(),
        src_id_expr: "src".to_string(),
        dst_id_expr: "dst".to_string(),
        // A catalog has many datasets/distributions per source vertex — keep all.
        single_valued: false,
        source_relation: values_relation("e", &["src", "dst"], &rows),
    })
}

/// Render `(VALUES (…), …) AS alias("c0", "c1", …)`. Rows must be non-empty and
/// each row must have one cell (a ready SQL literal) per column.
fn values_relation<S: AsRef<str>>(alias: &str, columns: &[S], rows: &[Vec<String>]) -> String {
    let cols = columns
        .iter()
        .map(|c| format!("\"{}\"", c.as_ref()))
        .collect::<Vec<_>>()
        .join(", ");
    let vals = rows
        .iter()
        .map(|r| format!("({})", r.join(", ")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("(VALUES {vals}) AS {alias}({cols})")
}

/// A single-quoted, escaped SQL string literal.
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// ── URN builders (ported verbatim from keasy's DCAT materializer) ─────────────
fn catalog_urn(job_id: &str) -> String {
    format!("urn:keasy:catalog:{job_id}")
}
fn dataset_urn(job_id: &str, type_name: &str) -> String {
    format!(
        "urn:keasy:dataset:{job_id}/{}",
        encode_uri_component(type_name)
    )
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
    s.replace(' ', "%20")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_run_status::{CatalogDataset, CatalogDistribution, CatalogField};

    fn sample() -> CatalogInput {
        CatalogInput {
            version: fossil_run_status::WIRE_VERSION,
            job_id: "job-1".to_string(),
            job_name: Some("My Run".to_string()),
            completed_at: "2026-06-02T00:00:00Z".to_string(),
            language: None,
            publisher_name: "Acme Org".to_string(),
            publisher_uri: None,
            catalog_description: Some("desc".to_string()),
            license_uri: Some("https://ex.org/license".to_string()),
            contact_email: Some("a@b.com".to_string()),
            datasets: vec![CatalogDataset {
                type_name: "Person".to_string(),
                source_name: Some("users".to_string()),
                rdf_type: Some("https://example.org/Person".to_string()),
                keywords: vec!["people".to_string()],
                entity_count: Some(5),
                fields: vec![CatalogField {
                    name: "name".to_string(),
                    rdf_uri: Some("https://example.org/name".to_string()),
                    datatype: Some("http://www.w3.org/2001/XMLSchema#string".to_string()),
                }],
                distributions: vec![CatalogDistribution {
                    destination: "s3://bucket/job-1/vertex/Person.parquet".to_string(),
                    media_type: "application/parquet".to_string(),
                }],
            }],
        }
    }

    #[test]
    fn builds_the_dcat_ap_vertex_and_edge_shape() {
        let (prelude, plan) = build_catalog_sink_plan(&sample());
        assert!(prelude.is_empty(), "inline VALUES → no prelude");

        let vtypes: Vec<&str> = plan.vertices.iter().map(|v| v.type_name.as_str()).collect();
        assert_eq!(
            vtypes,
            [
                "Catalog",
                "Dataset",
                "Distribution",
                "Agent",
                "Contact",
                "Field"
            ]
        );

        let etypes: Vec<(&str, &str, &str)> = plan
            .edges
            .iter()
            .map(|e| {
                (
                    e.src_type.as_str(),
                    e.predicate.as_str(),
                    e.dst_type.as_str(),
                )
            })
            .collect();
        assert_eq!(
            etypes,
            [
                ("Catalog", "dataset", "Dataset"),
                ("Dataset", "distribution", "Distribution"),
                ("Dataset", "field", "Field"),
                ("Catalog", "publisher", "Agent"),
                ("Catalog", "contactPoint", "Contact"),
            ]
        );
    }

    #[test]
    fn catalog_carries_rdf_spec_and_subject_urn() {
        let (_p, plan) = build_catalog_sink_plan(&sample());
        let catalog = &plan.vertices[0];
        assert_eq!(catalog.rdf_type.as_deref(), Some(DCAT_CATALOG));
        let title = catalog
            .properties
            .iter()
            .find(|p| p.name == "title")
            .unwrap();
        assert_eq!(title.rdf_uri.as_deref(), Some(DCT_TITLE));
        assert_eq!(title.xsd_datatype.as_deref(), Some(XSD_STRING));
        // The subject URN + escaped title literal land in the inline relation.
        assert!(
            catalog.source_relation.contains("urn:keasy:catalog:job-1"),
            "{}",
            catalog.source_relation
        );
        assert!(
            catalog.source_relation.contains("'My Run'"),
            "{}",
            catalog.source_relation
        );
    }

    #[test]
    fn dataset_entity_count_is_numeric_and_field_edges_resolve() {
        let (_p, plan) = build_catalog_sink_plan(&sample());
        let dataset = plan
            .vertices
            .iter()
            .find(|v| v.type_name == "Dataset")
            .unwrap();
        // entity_count is an unquoted integer literal in the VALUES.
        assert!(
            dataset.source_relation.contains(", 5)"),
            "{}",
            dataset.source_relation
        );
        let ecount = dataset
            .properties
            .iter()
            .find(|p| p.name == "entity_count")
            .unwrap();
        assert_eq!(
            ecount.rdf_uri, None,
            "entity_count is a data column, not RDF"
        );

        // The dataset→field edge points at the field URN via dst.
        let field_edge = plan.edges.iter().find(|e| e.predicate == "field").unwrap();
        assert!(
            field_edge
                .source_relation
                .contains("urn:keasy:field:job-1/Person/name"),
            "{}",
            field_edge.source_relation
        );
        assert_eq!(field_edge.src_id_expr, "src");
        assert_eq!(field_edge.dst_id_expr, "dst");
    }

    #[test]
    fn no_contact_when_email_absent() {
        let mut input = sample();
        input.contact_email = None;
        let (_p, plan) = build_catalog_sink_plan(&input);
        assert!(plan.vertices.iter().all(|v| v.type_name != "Contact"));
        assert!(plan.edges.iter().all(|e| e.predicate != "contactPoint"));
    }
}
