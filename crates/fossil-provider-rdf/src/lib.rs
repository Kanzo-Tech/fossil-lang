//! RDF source provider — `io.rdf("data.ttl", schema = "shape.shex")`.
//!
//! Implements the core's [`fossil_runtime::SourceProvider`] seam OUTSIDE the
//! language core: the core only emits a scan of the relation this provider
//! materialises and never sees a triple. A `ShEx` shape drives both halves —
//! the column schema (compile-time, [`describe`]) and the pivot of triples into
//! one wide row per subject (runtime, [`RdfProvider::materialize`]).
//!
//! v0.1 scope (documented, not hidden): a single-shape schema (the first shape
//! is used), and single-valued properties (the first object per
//! subject+predicate wins). Multi-valued (`*`) properties and multi-shape
//! schemas are follow-ups.

use std::collections::BTreeMap;

use duckdb::Connection;
use oxrdf::Term;
use oxttl::TurtleParser;

use fossil_descriptors_input::{InferredDescriptor, inferred_descriptor_from_shex};
use fossil_runtime::SourceProvider;
use fossil_shex::ShExDescriptor;

/// The RDF source provider. Stateless; register one instance per process.
#[derive(Debug, Default, Clone, Copy)]
pub struct RdfProvider;

/// The column the entity's subject IRI lands in (every materialised row carries
/// it, so a mapping can do `iri = .subject`).
const SUBJECT_COLUMN: &str = "subject";

impl SourceProvider for RdfProvider {
    fn name(&self) -> &'static str {
        "rdf"
    }

    fn extensions(&self) -> &[&str] {
        &["ttl", "nt", "n3", "rdf"]
    }

    fn materialize(
        &self,
        uri: &str,
        schema_arg: Option<&str>,
        relation: &str,
        conn: &Connection,
    ) -> Result<(), String> {
        let schema_path =
            schema_arg.ok_or_else(|| "io.rdf requires a `schema = \"<shape>.shex\"` argument".to_string())?;

        // The shape selects which predicates become columns (and their order).
        let columns = shape_columns(schema_path)?;

        // Read the RDF bytes through DuckDB's `read_text` on the passed
        // connection — NOT `std::fs` — so cloud `@conn` sources (Azure/S3) work
        // via the read creds already applied to the conn (httpfs/azure), exactly
        // like the native csv/parquet readers. Local paths + `file://` work too.
        let rdf_text = read_source_text(conn, uri)?;

        // Parse the RDF, grouping objects by subject then predicate IRI. BTreeMap
        // → deterministic subject + value order (stable output, testable).
        let mut by_subject: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for triple in TurtleParser::new().for_reader(rdf_text.as_bytes()) {
            let t = triple.map_err(|e| format!("parse RDF `{uri}`: {e}"))?;
            let subject = subject_value(&t.subject);
            let predicate = t.predicate.as_str().to_string();
            let object = term_value(&t.object);
            by_subject
                .entry(subject)
                .or_default()
                // single-valued (v0.1): first object per predicate wins.
                .entry(predicate)
                .or_insert(object);
        }

        conn.execute_batch(&create_table_sql(relation, &columns, &by_subject))
            .map_err(|e| format!("materialise RDF relation `{relation}`: {e}"))
    }
}

/// Derive the input [`InferredDescriptor`] for an `io.rdf` source from its `ShEx`
/// shape — the compile-time column schema.
///
/// Predicate local name → column, `valueExpr` → primitive. Used by the host
/// before type-checking. Uses the schema's first shape (v0.1 single-shape scope).
///
/// # Errors
///
/// Returns a message if the shape file is unreadable or has no shape.
pub fn describe(source_name: &str, schema_path: &str) -> Result<InferredDescriptor, String> {
    let bytes = read_shape(schema_path)?;
    let shape_iri = first_shape_iri(&bytes)?;
    inferred_descriptor_from_shex(source_name, &shape_iri, &bytes)
        .map_err(|e| format!("derive schema from `{schema_path}`: {e}"))
}

/// The ordered `(column_name, predicate_iri)` pairs for the shape's first shape.
fn shape_columns(schema_path: &str) -> Result<Vec<(String, String)>, String> {
    let bytes = read_shape(schema_path)?;
    let descriptor = ShExDescriptor::from_reader(bytes.as_slice())
        .map_err(|e| format!("parse ShEx `{schema_path}`: {e:?}"))?;
    let shape = descriptor
        .shapes()
        .next()
        .ok_or_else(|| format!("ShEx `{schema_path}` declares no shape"))?;
    Ok(shape
        .constraints
        .iter()
        .map(|c| (c.predicate_local_name(), c.predicate.to_string()))
        .collect())
}

fn first_shape_iri(shex_json: &[u8]) -> Result<String, String> {
    ShExDescriptor::from_reader(shex_json)
        .map_err(|e| format!("parse ShEx: {e:?}"))?
        .shapes()
        .next()
        .map(|s| s.iri.to_string())
        .ok_or_else(|| "ShEx declares no shape".to_string())
}

fn read_shape(schema_path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(schema_path).map_err(|e| format!("read ShEx `{schema_path}`: {e}"))
}

/// Read an RDF source's full text via DuckDB's `read_text` on `conn`. This is
/// the cloud-capable read path: a `@conn` source resolved to `az://…` / `s3://…`
/// is fetched through the read creds already installed on the connection (the
/// same httpfs/azure path the native readers use); local paths + `file://` work
/// unchanged. The whole document is read into memory (v0.1; streaming is a
/// follow-up for larger-than-RAM RDF).
fn read_source_text(conn: &Connection, uri: &str) -> Result<String, String> {
    conn.query_row(
        &format!("SELECT content FROM read_text({})", sql_str(uri)),
        [],
        |row| row.get::<_, String>(0),
    )
    .map_err(|e| format!("read RDF source `{uri}` via DuckDB read_text: {e}"))
}

/// The bare IRI (or blank-node label) of a triple subject — `as_str`, NOT the
/// `Display` form (which wraps IRIs in `<>`).
fn subject_value(subject: &oxrdf::NamedOrBlankNode) -> String {
    match subject {
        oxrdf::NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        oxrdf::NamedOrBlankNode::BlankNode(b) => b.to_string(),
    }
}

/// The lexical value of an RDF object term: a literal's lexical form, or a
/// node's IRI. Blank nodes render as their `_:id` label.
fn term_value(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::BlankNode(b) => b.to_string(),
        // RDF-star quoted triple — out of scope; keep its surface form.
        Term::Triple(_) => term.to_string(),
    }
}

/// Build a `CREATE OR REPLACE TABLE` statement for the pivoted entities. All
/// columns are `VARCHAR` (`DuckDB` / the mapping cast as needed); the `subject`
/// column carries the entity IRI. Empty input yields a typed empty table.
fn create_table_sql(
    relation: &str,
    columns: &[(String, String)],
    by_subject: &BTreeMap<String, BTreeMap<String, String>>,
) -> String {
    let col_names: Vec<String> = std::iter::once(SUBJECT_COLUMN.to_string())
        .chain(columns.iter().map(|(name, _)| name.clone()))
        .collect();
    let quoted_cols = col_names
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");

    if by_subject.is_empty() {
        let decls = col_names
            .iter()
            .map(|c| format!("\"{c}\" VARCHAR"))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("CREATE OR REPLACE TABLE \"{relation}\" ({decls});");
    }

    let rows = by_subject
        .iter()
        .map(|(subject, props)| {
            let mut cells = vec![sql_str(subject)];
            cells.extend(
                columns
                    .iter()
                    .map(|(_, iri)| sql_str(props.get(iri).map_or("", String::as_str))),
            );
            format!("({})", cells.join(", "))
        })
        .collect::<Vec<_>>()
        .join(",\n  ");

    format!(
        "CREATE OR REPLACE TABLE \"{relation}\" AS \
         SELECT * FROM (VALUES\n  {rows}\n) AS t({quoted_cols});"
    )
}

/// A single-quoted SQL string literal with `'` escaped.
fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERSON_SHEX: &str = r#"{
      "@context": "http://www.w3.org/ns/shex.jsonld",
      "type": "Schema",
      "shapes": [
        {
          "type": "ShapeDecl",
          "id": "http://example.org/Person",
          "shapeExpr": {
            "type": "Shape",
            "expression": {
              "type": "EachOf",
              "expressions": [
                { "type": "TripleConstraint", "predicate": "http://xmlns.com/foaf/0.1/name",
                  "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#string" } },
                { "type": "TripleConstraint", "predicate": "http://xmlns.com/foaf/0.1/age",
                  "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#integer" } }
              ]
            }
          }
        }
      ]
    }"#;

    const PEOPLE_TTL: &str = r#"@prefix foaf: <http://xmlns.com/foaf/0.1/> .
        <http://example.org/p/1> foaf:name "Alice" ; foaf:age 30 .
        <http://example.org/p/2> foaf:name "Bob" ; foaf:age 25 .
    "#;

    fn write_tmp(name: &str, content: &str) -> String {
        let path = std::env::temp_dir().join(format!("fossil-rdf-test-{name}"));
        std::fs::write(&path, content).expect("write tmp");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn describe_derives_columns_from_the_shape() {
        let shex = write_tmp("describe.shex", PERSON_SHEX);
        let d = describe("people", &shex).expect("describe");
        let cols: Vec<&str> = d.columns.iter().map(|c| c.name.as_str()).collect();
        assert!(cols.contains(&"name"));
        assert!(cols.contains(&"age"));
        let age = d.columns.iter().find(|c| c.name == "age").unwrap();
        assert_eq!(age.primitive.as_str(), "Integer");
    }

    #[test]
    fn materialize_pivots_triples_into_entity_rows() {
        let shex = write_tmp("mat.shex", PERSON_SHEX);
        let ttl = write_tmp("mat.ttl", PEOPLE_TTL);
        let conn = Connection::open_in_memory().expect("duckdb");

        RdfProvider
            .materialize(&ttl, Some(&shex), "__fossil_src_people", &conn)
            .expect("materialize");

        // One wide row per subject, columns = subject + shape predicates.
        let n: i64 = conn
            .query_row("SELECT count(*) FROM \"__fossil_src_people\"", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 2);

        let name: String = conn
            .query_row(
                "SELECT name FROM \"__fossil_src_people\" WHERE subject = 'http://example.org/p/1'",
                [],
                |r| r.get(0),
            )
            .expect("row");
        assert_eq!(name, "Alice");

        let age: String = conn
            .query_row(
                "SELECT age FROM \"__fossil_src_people\" WHERE subject = 'http://example.org/p/2'",
                [],
                |r| r.get(0),
            )
            .expect("row");
        assert_eq!(age, "25");
    }
}
