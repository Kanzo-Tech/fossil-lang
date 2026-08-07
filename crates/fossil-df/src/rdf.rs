//! RDF source provider as Arrow — the universal substrate's **input seam** for
//! `io.rdf`. Parses Turtle into triples and pivots them to one wide row per
//! subject of a shape: subjects are selected by `rdf:type == shape_iri`, and the
//! columns are the shape's predicates. The result is a [`RecordBatch`] the host
//! registers as a table, so the executor scans RDF exactly like a CSV — **RDF
//! stays at the I/O border, the core never sees it** (design §C2; this is the
//! DataFusion-native, WASM-clean counterpart of `fossil-provider-rdf`, which
//! pivots into DuckDB).
//!
//! v1 covers single-valued (scalar) columns; multi-valued predicates (ShEx
//! `*`/`+` → an Arrow `List`) are a follow-up. `oxttl`/`oxrdf` are pure Rust, so
//! this compiles to wasm and decodes RDF in the browser too.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, ListBuilder, StringArray, StringBuilder};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::error::DataFusionError;
use oxrdf::{NamedOrBlankNode, Term};
use oxttl::TurtleParser;

/// The `rdf:type` predicate — the sole subject selector (a shape's rows are the
/// subjects typed with the shape's IRI), matching `fossil-provider-rdf`.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// One pivoted column: the relation column `name` carries the object(s) of
/// `predicate` for each subject. `multi` (a `*`/`+` ShEx cardinality) pivots into
/// an Arrow `List<Utf8>` of every object — the source of a multi-valued vertex
/// property or, after UNNEST, a multi-valued edge; a single-valued column is a
/// scalar `Utf8` (the deterministic minimum object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdfColumn {
    pub name: String,
    pub predicate: String,
    pub multi: bool,
}

/// Parse `turtle` and pivot the subjects of `type_iri` (those with an
/// `rdf:type` triple to it) into one wide row per subject: a `subject` IRI
/// column + one column per [`RdfColumn`] (the object of that predicate, or
/// null). Rows are ordered by subject IRI so the downstream dense id is
/// deterministic.
///
/// # Errors
/// Turtle parse errors or Arrow construction errors.
pub fn rdf_to_batch(
    turtle: &str,
    type_iri: &str,
    columns: &[RdfColumn],
) -> datafusion::error::Result<RecordBatch> {
    // subject → predicate → ALL objects. A single-valued column later takes the
    // minimum (deterministic); a multi-valued one keeps them all as a List.
    let mut rows: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut typed: BTreeSet<String> = BTreeSet::new();

    let mut parser = TurtleParser::new().for_reader(turtle.as_bytes());
    for triple in parser.by_ref() {
        let t = triple.map_err(|e| DataFusionError::Execution(format!("parse RDF: {e}")))?;
        let subject = subject_value(&t.subject);
        let predicate = t.predicate.as_str();
        let object = term_value(&t.object);
        if predicate == RDF_TYPE && object == type_iri {
            typed.insert(subject.clone());
        }
        rows.entry(subject)
            .or_default()
            .entry(predicate.to_string())
            .or_default()
            .push(object);
    }

    // The shape's subjects, sorted (BTreeSet iterates in order).
    let subjects: Vec<&String> = typed.iter().collect();

    let subject_col: StringArray = subjects.iter().map(|s| Some(s.as_str())).collect();
    let mut fields = vec![Field::new("subject", DataType::Utf8, false)];
    let mut arrays: Vec<ArrayRef> = vec![Arc::new(subject_col)];

    for column in columns {
        let objects_of = |s: &String| rows.get(s).and_then(|preds| preds.get(&column.predicate));
        if column.multi {
            // List<Utf8> of every object (sorted per subject for determinism); a
            // subject with no object for this predicate gets an empty list (→ no
            // edges / no values after UNNEST).
            let mut builder = ListBuilder::new(StringBuilder::new());
            for s in &subjects {
                if let Some(objs) = objects_of(s) {
                    let mut objs: Vec<&String> = objs.iter().collect();
                    objs.sort();
                    for o in objs {
                        builder.values().append_value(o);
                    }
                }
                builder.append(true);
            }
            fields.push(Field::new(
                &column.name,
                DataType::List(Arc::new(Field::new_list_field(DataType::Utf8, true))),
                true,
            ));
            arrays.push(Arc::new(builder.finish()));
        } else {
            // Single-valued: the minimum object (deterministic; for valid data
            // there is exactly one per subject).
            let values: StringArray = subjects
                .iter()
                .map(|s| {
                    objects_of(s)
                        .and_then(|v| v.iter().min())
                        .map(String::as_str)
                })
                .collect();
            fields.push(Field::new(&column.name, DataType::Utf8, true));
            arrays.push(Arc::new(values));
        }
    }

    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(Into::into)
}

/// The bare IRI (or blank-node label) of a triple subject — `as_str`, NOT the
/// `Display` form (which wraps IRIs in `<>`).
fn subject_value(subject: &NamedOrBlankNode) -> String {
    match subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNode::BlankNode(b) => b.to_string(),
    }
}

/// The lexical value of an RDF object term: a literal's lexical form, or a
/// node's IRI. Blank nodes render as their `_:id` label.
fn term_value(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::BlankNode(b) => b.to_string(),
        Term::Triple(_) => term.to_string(), // RDF-star quoted triple — surface form
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::Array;

    const TURTLE: &str = r#"
        @prefix ex:   <https://example.org/> .
        @prefix foaf: <http://xmlns.com/foaf/0.1/> .
        ex:alice a ex:Person ; foaf:name "Alice" ; foaf:age "30" .
        ex:bob   a ex:Person ; foaf:name "Bob" .
        ex:acme  a ex:Org    ; foaf:name "Acme" .
    "#;

    #[test]
    fn pivots_a_shape_to_wide_rows() {
        let columns = [
            RdfColumn {
                name: "name".into(),
                predicate: "http://xmlns.com/foaf/0.1/name".into(),
                multi: false,
            },
            RdfColumn {
                name: "age".into(),
                predicate: "http://xmlns.com/foaf/0.1/age".into(),
                multi: false,
            },
        ];
        let batch = rdf_to_batch(TURTLE, "https://example.org/Person", &columns).expect("pivot");

        // Only the two Persons (Acme is an Org), sorted by subject IRI.
        assert_eq!(batch.num_rows(), 2);
        let schema = batch.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, ["subject", "name", "age"]);

        let col = |i: usize| {
            let a = batch
                .column(i)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            (0..a.len())
                .map(|r| {
                    if a.is_null(r) {
                        None
                    } else {
                        Some(a.value(r).to_string())
                    }
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            col(0),
            [
                Some("https://example.org/alice".into()),
                Some("https://example.org/bob".into())
            ],
        );
        assert_eq!(col(1), [Some("Alice".into()), Some("Bob".into())]);
        // Bob has no foaf:age → null.
        assert_eq!(col(2), [Some("30".into()), None]);
    }

    const MULTI_TTL: &str = r#"@prefix ex: <https://ex.org/> .
        <https://ex.org/kb/1> a ex:KB ; ex:hasProject <https://ex.org/proj/2>, <https://ex.org/proj/1> .
        <https://ex.org/kb/2> a ex:KB .
    "#;

    #[test]
    fn multi_valued_predicate_pivots_to_a_list() {
        use datafusion::arrow::array::{Array, ListArray};

        let columns = [RdfColumn {
            name: "hasProject".into(),
            predicate: "https://ex.org/hasProject".into(),
            multi: true,
        }];
        let batch = rdf_to_batch(MULTI_TTL, "https://ex.org/KB", &columns).expect("pivot");

        assert_eq!(batch.num_rows(), 2, "two KBs");
        let list = batch
            .column(1)
            .as_any()
            .downcast_ref::<ListArray>()
            .expect("List col");

        // kb/1 → its two projects, sorted (proj/1 before proj/2 despite TTL order).
        let row0 = list.value(0);
        let row0 = row0.as_any().downcast_ref::<StringArray>().unwrap();
        let got: Vec<&str> = (0..row0.len()).map(|i| row0.value(i)).collect();
        assert_eq!(got, ["https://ex.org/proj/1", "https://ex.org/proj/2"]);

        // kb/2 → empty list (no projects → no edges after UNNEST).
        assert_eq!(list.value(1).len(), 0, "kb/2 has no projects");
    }
}
