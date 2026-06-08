//! RDF source provider — `{ A, B, ... } := io.rdf("data.ttl", schema = "x.shex")`.
//!
//! Implements the core's [`fossil_runtime::SourceProvider`] seam OUTSIDE the
//! language core: the core only emits a scan of the relations this provider
//! materialises and never sees a triple. A `ShEx` shape drives both halves —
//! the column schema (compile-time, in `fossil-hir`) and the pivot of triples
//! into one wide row per subject (runtime,
//! [`RdfProvider::materialize_shapes`]).
//!
//! Single path: the `.ttl` is read + parsed ONCE per `io.rdf` call, yielding N
//! typed relations (one per destructured member). Subject selection is ALWAYS by
//! `rdf:type`: shape S's rows are the subjects with a triple `(s, rdf:type,
//! <IRI of S>)`. Not configurable; there are NO `ShapeMaps`. Multi-valued (`*`/`+`)
//! predicates pivot into a `DuckDB` `LIST` (the output edge decomposition
//! `UNNEST`s them).

use std::collections::BTreeSet;

use duckdb::Connection;
use oxrdf::Term;
use oxttl::TurtleParser;

use fossil_runtime::SourceProvider;
use fossil_shex::ShExDescriptor;

/// The RDF source provider. Stateless; register one instance per process.
#[derive(Debug, Default, Clone, Copy)]
pub struct RdfProvider;

/// The column the entity's subject IRI lands in (every materialised row carries
/// it, so a mapping can do `iri = .subject`).
const SUBJECT_COLUMN: &str = "subject";

/// The `rdf:type` predicate — the SOLE subject selector (a shape's rows are the
/// subjects typed with the shape's IRI). Not configurable.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

impl SourceProvider for RdfProvider {
    fn name(&self) -> &'static str {
        "rdf"
    }

    fn extensions(&self) -> &[&str] {
        &["ttl", "nt", "n3", "rdf"]
    }

    fn materialize_shapes(
        &self,
        uri: &str,
        schema_arg: Option<&str>,
        members: &[(String, String)],
        conn: &Connection,
    ) -> Result<(), String> {
        // The schema arrives as a RESOLVED locator (the host resolved any `@conn`
        // alias). Read through the SAME DuckDB `read_text` on the connection —
        // never `std::fs` — so a cloud-hosted `@conn` schema works via the creds
        // already applied to the conn, identical to the data URI below.
        let schema_locator = schema_arg
            .ok_or_else(|| "io.rdf requires a `schema = \"<shape>.shex\"` argument".to_string())?;

        // Parse the ShEx ONCE: it drives the columns (which shape → which columns).
        let schema_text = read_source_text(conn, schema_locator)?;
        let descriptor = ShExDescriptor::from_reader(schema_text.as_bytes())
            .map_err(|e| format!("parse ShEx `{schema_locator}`: {e:?}"))?;

        // The RDF data, read through the same `read_text` path as the schema —
        // one cloud-capable reader for every reference. Parse it ONCE into a flat
        // triple list, shared by every member's pivot below.
        let rdf_text = read_source_text(conn, uri)?;
        let mut parser = TurtleParser::new().for_reader(rdf_text.as_bytes());
        let mut triples: Vec<(String, String, String)> = Vec::new();
        for triple in parser.by_ref() {
            let t = triple.map_err(|e| format!("parse RDF `{uri}`: {e}"))?;
            triples.push((
                subject_value(&t.subject),
                t.predicate.as_str().to_string(),
                term_value(&t.object),
            ));
        }

        // One relation per member: select its subjects by `rdf:type == shape`,
        // derive its columns from the shape, pivot. The triple list is parsed
        // once and reused across members.
        for (relation, shape_iri) in members {
            let columns = shape_columns(&descriptor, shape_iri)?;
            let selected = select_by_type(&triples, shape_iri);
            write_pivoted_relation(conn, relation, &columns, &triples, &selected)?;
        }
        Ok(())
    }
}

/// The subjects of a shape: every subject carrying `(s, rdf:type, shape_iri)`.
/// The SOLE selection rule — not configurable, no `ShapeMaps`.
fn select_by_type(triples: &[(String, String, String)], shape_iri: &str) -> BTreeSet<String> {
    triples
        .iter()
        .filter(|(_, p, o)| p == RDF_TYPE && o == shape_iri)
        .map(|(s, _, _)| s.clone())
        .collect()
}

/// The ordered `(column_name, predicate_iri)` pairs for `shape_iri` — the
/// member's shape — so a multi-shape schema yields the columns of THIS member's
/// class (shapes live in a `HashMap`, so `.next()` order is non-deterministic).
fn shape_columns(descriptor: &ShExDescriptor, shape_iri: &str) -> Result<Vec<ShapeColumn>, String> {
    let shape = descriptor
        .lookup_shape_str(shape_iri)
        .ok_or_else(|| format!("ShEx has no shape `{shape_iri}`"))?;
    Ok(shape
        .constraints
        .iter()
        .map(|c| ShapeColumn {
            name: c.predicate_local_name(),
            predicate: c.predicate.to_string(),
            // Multi-valued (`*`/`+`) predicates pivot into a DuckDB `LIST`; the
            // single source of this rule is shared with the output decomposition.
            single_valued: c.cardinality.is_single_valued(),
        })
        .collect())
}

/// One pivoted column the shape declares: a predicate → relation column, plus
/// whether it is single-valued (one object → scalar) or multi-valued (→ `LIST`).
struct ShapeColumn {
    name: String,
    predicate: String,
    single_valued: bool,
}

/// Read an RDF source's full text via `DuckDB`'s `read_text` on `conn`. This is
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

/// Pivot the selected subjects' triples into one wide row per subject. Loads the
/// triples into a `DuckDB` temp table via the **Appender** (parameterized — no
/// hand-built `VALUES` SQL) then pivots with conditional aggregation: each shape
/// predicate becomes a column, a predicate absent for a subject yields NULL. All
/// columns are `VARCHAR` (the mapping casts as needed); the `subject` column
/// carries the entity IRI. v0.1 is single-valued (one object per predicate wins
/// via `MAX`); multi-valued + edge columns are the next increment.
fn write_pivoted_relation(
    conn: &Connection,
    relation: &str,
    columns: &[ShapeColumn],
    triples: &[(String, String, String)],
    selected: &BTreeSet<String>,
) -> Result<(), String> {
    let subj = SUBJECT_COLUMN;
    let staging = format!("__rdf_triples_{relation}");

    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE \"{staging}\" ({subj} VARCHAR, predicate VARCHAR, object VARCHAR);"
    ))
    .map_err(|e| format!("create RDF triples staging table: {e}"))?;

    {
        let mut appender = conn
            .appender(&staging)
            .map_err(|e| format!("open appender for `{staging}`: {e}"))?;
        for (s, p, o) in triples {
            if selected.contains(s) {
                appender
                    .append_row(duckdb::params![s, p, o])
                    .map_err(|e| format!("append RDF triple: {e}"))?;
            }
        }
        // Appender flushes on drop (end of this scope).
    }

    let projection = std::iter::once(format!("\"{subj}\""))
        .chain(columns.iter().map(|col| {
            let ShapeColumn {
                name,
                predicate,
                single_valued,
            } = col;
            let pred = sql_str(predicate);
            if *single_valued {
                // One object wins (deterministic via MAX).
                format!("MAX(object) FILTER (WHERE predicate = {pred}) AS \"{name}\"")
            } else {
                // Multi-valued → a LIST cell (the output edge decomposition UNNESTs
                // it into one edge per element). Deterministic order for parity.
                format!("list(object ORDER BY object) FILTER (WHERE predicate = {pred}) AS \"{name}\"")
            }
        }))
        .collect::<Vec<_>>()
        .join(", ");

    conn.execute_batch(&format!(
        "CREATE OR REPLACE TABLE \"{relation}\" AS \
         SELECT {projection} FROM \"{staging}\" GROUP BY \"{subj}\" ORDER BY \"{subj}\"; \
         DROP TABLE \"{staging}\";"
    ))
    .map_err(|e| format!("materialise RDF relation `{relation}`: {e}"))
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
        <http://example.org/p/1> a <http://example.org/Person> ; foaf:name "Alice" ; foaf:age 30 .
        <http://example.org/p/2> a <http://example.org/Person> ; foaf:name "Bob" ; foaf:age 25 .
        <http://example.org/o/1> a <http://example.org/Org> ; foaf:name "Acme" .
    "#;

    fn write_tmp(name: &str, content: &str) -> String {
        let path = std::env::temp_dir().join(format!("fossil-rdf-test-{name}"));
        std::fs::write(&path, content).expect("write tmp");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn select_by_type_keeps_only_subjects_typed_with_the_shape() {
        // The SOLE selection rule: `(s, rdf:type, shape_iri)`. No ShapeMaps.
        let triples = vec![
            (
                "http://example.org/p/1".to_string(),
                RDF_TYPE.to_string(),
                "http://example.org/Person".to_string(),
            ),
            (
                "http://example.org/p/1".to_string(),
                "http://xmlns.com/foaf/0.1/name".to_string(),
                "Alice".to_string(),
            ),
            (
                "http://example.org/o/1".to_string(),
                RDF_TYPE.to_string(),
                "http://example.org/Org".to_string(),
            ),
        ];
        let sel = select_by_type(&triples, "http://example.org/Person");
        assert!(sel.contains("http://example.org/p/1"), "Person selected");
        assert!(!sel.contains("http://example.org/o/1"), "Org excluded");
        assert_eq!(sel.len(), 1);
    }

    #[test]
    fn materialize_shapes_reads_once_and_pivots_each_shape() {
        let shex = write_tmp("mat.shex", PERSON_SHEX);
        let ttl = write_tmp("mat.ttl", PEOPLE_TTL);
        let conn = Connection::open_in_memory().expect("duckdb");

        // One io.rdf call, one member (`Person`) — selected by rdf:type.
        RdfProvider
            .materialize_shapes(
                &ttl,
                Some(&shex),
                &[(
                    "__fossil_src_people".to_string(),
                    "http://example.org/Person".to_string(),
                )],
                &conn,
            )
            .expect("materialize_shapes");

        // 3 subjects in the doc, 2 typed Person → 2 rows; the Org is excluded.
        let n: i64 = conn
            .query_row("SELECT count(*) FROM \"__fossil_src_people\"", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 2);

        let orgs: i64 = conn
            .query_row(
                "SELECT count(*) FROM \"__fossil_src_people\" WHERE subject = 'http://example.org/o/1'",
                [],
                |r| r.get(0),
            )
            .expect("count orgs");
        assert_eq!(orgs, 0);

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
