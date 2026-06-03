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

use std::collections::BTreeSet;

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
        select_arg: Option<&str>,
        relation: &str,
        conn: &Connection,
    ) -> Result<(), String> {
        let schema_path =
            schema_arg.ok_or_else(|| "io.rdf requires a `schema = \"<shape>.shex\"` argument".to_string())?;
        // Entity selection is DECLARED, not inferred: a ShEx ShapeMap states which
        // RDF nodes become rows. Mandatory — no hidden engine rule (e.g. no implicit
        // `rdf:type` match); `rdf:type` is just one selector the ShapeMap may declare.
        let select_path = select_arg.ok_or_else(|| {
            "io.rdf requires a `select = \"<shapemap>.smap\"` argument (a ShEx ShapeMap declaring which RDF nodes become entities)".to_string()
        })?;
        let shapemap_src = read_text_file(select_path)?;

        // The shape selects which predicates become columns (and their order).
        let columns = shape_columns(schema_path)?;

        // Read the RDF bytes through DuckDB's `read_text` on the passed
        // connection — NOT `std::fs` — so cloud `@conn` sources (Azure/S3) work
        // via the read creds already applied to the conn (httpfs/azure), exactly
        // like the native csv/parquet readers. Local paths + `file://` work too.
        let rdf_text = read_source_text(conn, uri)?;

        // Parse the RDF into a flat triple list — both for evaluating the ShapeMap
        // selectors and for the SQL pivot (loaded via the Appender, not hand-built
        // SQL).
        let mut triples: Vec<(String, String, String)> = Vec::new();
        for triple in TurtleParser::new().for_reader(rdf_text.as_bytes()) {
            let t = triple.map_err(|e| format!("parse RDF `{uri}`: {e}"))?;
            triples.push((
                subject_value(&t.subject),
                t.predicate.as_str().to_string(),
                term_value(&t.object),
            ));
        }

        // Keep only the subjects the ShapeMap selects — declared, not a magic
        // `rdf:type` rule. A document with 1564 subjects but ~21 selected emits ~21.
        let selected = select_subjects(&shapemap_src, &triples)?;

        write_pivoted_relation(conn, relation, &columns, &triples, &selected)
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
fn shape_columns(schema_path: &str) -> Result<Vec<ShapeColumn>, String> {
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

/// Read a local text file (the ShapeMap `.smap`). Local-only in v0.1 (the shape
/// + shapemap are program-side artifacts, like `read_shape`); cloud-hosted
/// selection artifacts are a follow-up.
fn read_text_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("read `{path}`: {e}"))
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

/// Pivot the selected subjects' triples into one wide row per subject. Loads the
/// triples into a DuckDB temp table via the **Appender** (parameterized — no
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

/// Evaluate a ShEx **ShapeMap** (compact syntax) against the parsed triples to
/// select which subjects become entity rows. The user declares the subset
/// explicitly — `{FOCUS a <Class>}@<Shape>` or an explicit node — so `rdf:type`
/// is one possible *declared* selector, not a hardcoded engine rule.
///
/// We use rudof's ShapeMap **parser** (`shex_ast::ShapeMapParser`, WASM-safe)
/// for authoring fidelity, but evaluate the selectors ourselves against the
/// oxttl triples: rudof 0.3.1's resolver (`node_shapes`) routes through
/// SPARQL/oxigraph, which is unwired for in-memory graphs AND not WASM-safe.
///
/// v0.1 supports two selector forms (full IRIs; prefixes are a follow-up):
/// an explicit node, and `{FOCUS <pred> <class>}` (including `a` = `rdf:type`).
fn select_subjects(
    shapemap_src: &str,
    triples: &[(String, String, String)],
) -> Result<BTreeSet<String>, String> {
    use shex_ast::ObjectValue;
    use shex_ast::ShapeMapParser;
    use shex_ast::shapemap::{NodeSelector, Pattern, SHACLPathRef};

    let qsm = ShapeMapParser::parse(shapemap_src, &None, &None, &None, &None)
        .map_err(|e| format!("parse ShapeMap: {e}"))?;

    let mut selected = BTreeSet::new();
    for assoc in qsm.iter() {
        match &assoc.node_selector {
            // Explicit node: keep that subject if it occurs in the data.
            NodeSelector::Node(ObjectValue::IriRef(iri)) => {
                let node = iri_bare(&iri.to_string());
                if triples.iter().any(|(s, _, _)| *s == node) {
                    selected.insert(node);
                }
            }
            // `{FOCUS <pred> <class>}` (incl. `{FOCUS a <Class>}`): keep every
            // subject carrying the triple (subject, pred, class).
            NodeSelector::TriplePattern {
                subject: Pattern::Focus,
                path: SHACLPathRef::Predicate { pred },
                object: Pattern::Node(ObjectValue::IriRef(class)),
            } => {
                let p = iri_bare(&pred.to_string());
                let c = iri_bare(&class.to_string());
                for (s, tp, to) in triples {
                    if *tp == p && *to == c {
                        selected.insert(s.clone());
                    }
                }
            }
            other => {
                return Err(format!(
                    "unsupported ShapeMap selector in v0.1 (use an explicit node or `{{FOCUS <pred> <class>}}`): {other:?}"
                ));
            }
        }
    }
    Ok(selected)
}

/// Strip the surrounding `<>` an IRI carries in its display form, leaving the
/// bare IRI to compare against oxttl's `as_str()` subject/predicate/object text.
fn iri_bare(s: &str) -> String {
    s.trim_start_matches('<').trim_end_matches('>').to_string()
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
    fn shapemap_type_pattern_selects_only_that_class() {
        // `{FOCUS a <Class>}` declared selector — rudof parses it, we evaluate it.
        let smap = "{FOCUS a <http://example.org/Person>}@<http://example.org/PersonShape>";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type".to_string();
        let triples = vec![
            (
                "http://example.org/p/1".to_string(),
                rdf_type.clone(),
                "http://example.org/Person".to_string(),
            ),
            (
                "http://example.org/p/1".to_string(),
                "http://xmlns.com/foaf/0.1/name".to_string(),
                "Alice".to_string(),
            ),
            (
                "http://example.org/o/1".to_string(),
                rdf_type,
                "http://example.org/Org".to_string(),
            ),
        ];
        let sel = select_subjects(smap, &triples).expect("select");
        assert!(sel.contains("http://example.org/p/1"), "Person selected");
        assert!(!sel.contains("http://example.org/o/1"), "Org excluded");
        assert_eq!(sel.len(), 1);
    }

    #[test]
    fn shapemap_explicit_node_selects_that_subject() {
        let smap = "<http://example.org/p/2>@<http://example.org/PersonShape>";
        let triples = vec![
            (
                "http://example.org/p/2".to_string(),
                "http://xmlns.com/foaf/0.1/name".to_string(),
                "Bob".to_string(),
            ),
            (
                "http://example.org/p/9".to_string(),
                "http://xmlns.com/foaf/0.1/name".to_string(),
                "Nobody".to_string(),
            ),
        ];
        let sel = select_subjects(smap, &triples).expect("select");
        assert_eq!(sel.len(), 1);
        assert!(sel.contains("http://example.org/p/2"));
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
        // The ShapeMap declares the subset: nodes typed as Person.
        let smap = write_tmp(
            "mat.smap",
            "{FOCUS a <http://example.org/Person>}@<http://example.org/Person>",
        );
        let conn = Connection::open_in_memory().expect("duckdb");

        RdfProvider
            .materialize(&ttl, Some(&shex), Some(&smap), "__fossil_src_people", &conn)
            .expect("materialize");

        // One wide row per subject the ShapeMap selects — the `Org` subject is
        // not selected, so 3 subjects yield 2 rows.
        let n: i64 = conn
            .query_row("SELECT count(*) FROM \"__fossil_src_people\"", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 2);

        // The off-class subject (`o/1`, an Org) is excluded.
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
