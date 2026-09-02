//! RDF source provider as Arrow — the universal substrate's **input seam** for
//! `io.rdf`. Parses Turtle into triples and pivots them to one wide row per
//! subject of a shape: subjects are selected by `rdf:type == shape_iri`, and the
//! columns are the shape's predicates. The result is a [`RecordBatch`] the host
//! registers as a table, so the executor scans RDF exactly like a CSV — **RDF
//! stays at the I/O border, the core never sees it**. This is the
//! DataFusion-native, WASM-clean pivot, and there is no other — no
//! `SourceProvider` trait and no `fossil-provider-rdf` crate, which
//! `fossil-base`'s `RowReader::Materialised` also says outright.
//!
//! A column is scalar or multi-valued: a ShEx `*`/`+` cardinality pivots into an
//! Arrow `List<Utf8>` of every object, a single-valued one into a `Utf8` cell.
//! `oxttl`/`oxrdf` are pure Rust, so this compiles to wasm and decodes RDF in the
//! browser too.
//!
//! **RDF 1.2.** The parser is an RDF 1.2 one (`oxttl`'s `rdf-12` feature, turned
//! on in the workspace manifest with the measurement behind it). Nothing in RDF
//! 1.2 is a Recommendation and every concrete syntax is still a Working Draft, so
//! what we lean on is the abstract model, not the spelling: a triple term reaches
//! [`term_value`] as `Term::Triple` and a base direction as
//! `Literal::direction`. Neither survives the pivot, and the second one says so
//! rather than answering wrongly.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, ListBuilder, StringArray, StringBuilder};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::error::DataFusionError;
use fossil_descriptors_output::subject_value;
use oxrdf::{Literal, Term};
use oxttl::TurtleParser;

/// The `rdf:type` predicate — the sole subject selector: a shape's rows are the
/// subjects typed with the shape's IRI.
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
        if let Term::Literal(l) = &t.object
            && l.direction().is_some()
        {
            return Err(directional_literal_unsupported(&subject, predicate, l));
        }
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

/// The lexical value of an RDF object term: a literal's lexical form, or a
/// node's IRI. Blank nodes render as their `_:id` label.
///
/// Lexical form ONLY — a language tag does not survive this, and neither would a
/// base direction, which is why [`rdf_to_batch`] refuses one before getting here.
fn term_value(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::BlankNode(b) => b.to_string(),
        // An RDF 1.2 triple term. It has no identity of its own (Concepts §3.1)
        // — the identity belongs to the REIFIER — so there is nothing to key a
        // row by; the surface form is a placeholder. A reifier needs no new
        // syntax, because an IRI with properties is already a mapping; what it
        // needs is a lowering, and that does not exist yet.
        Term::Triple(_) => term.to_string(),
    }
}

/// A directional language-tagged string (`"x"@he--rtl`, datatype
/// `rdf:dirLangString`) reached a pivot column.
///
/// The parser keeps the direction — `rdf-12` is on — and the pivot has nowhere to
/// put it: a column is one `Utf8` cell holding a lexical form, and `Primitive`,
/// the one datatype lattice, has no `dirLangString`. Returning `"x"` for
/// `"x"@he--rtl` is not a lossy convenience, it is the wrong string: base
/// direction exists because the first strong character does not determine how the
/// text is laid out, so an RTL value rendered as LTR reorders in the host. So the
/// read path names the loss instead of committing it. What would lift it is a
/// column type that carries the direction, and that is a new `Primitive` — which
/// waits for a producer that emits one, because there is no Turtle writer, no
/// descriptor that declares a `dirLangString`, and adding a lattice variant that
/// nobody constructs is the abstraction-before-the-second-implementation this
/// tree refuses everywhere else.
fn directional_literal_unsupported(
    subject: &str,
    predicate: &str,
    literal: &Literal,
) -> DataFusionError {
    DataFusionError::NotImplemented(format!(
        "RDF 1.2 base direction on <{subject}> <{predicate}>: {literal} is an \
         rdf:dirLangString, and a pivot column carries a lexical form with no \
         direction — see /docs/characteristics/rdf12"
    ))
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

    // ── RDF 1.2 ────────────────────────────────────────────────────────────
    //
    // Both of these are parse-level: they assert what the `rdf-12` feature buys,
    // and each fails with a *parse error from oxttl* the moment the feature goes
    // off — which is the failure the workspace manifest's comment exists to
    // prevent. Measured off-feature (oxttl 0.2.3, 2026-08-07): "`<<(` is not a
    // valid RDF object" and "Literal base direction are only allowed in RDF 1.2".

    #[test]
    fn a_triple_term_parses() {
        const TTL: &str = r#"@prefix ex: <https://ex.org/> .
            ex:claim a ex:Reifier ;
                     ex:reifies <<( ex:alice ex:knows ex:bob )>> .
        "#;

        // The object-position-only rule of Concepts §3.1, as the parser enforces it.
        let terms: Vec<Term> = TurtleParser::new()
            .for_reader(TTL.as_bytes())
            .map(|t| t.expect("RDF 1.2 Turtle parses").object)
            .collect();
        let quoted = terms
            .iter()
            .find_map(|t| match t {
                Term::Triple(inner) => Some(inner),
                _ => None,
            })
            .expect("one triple term");
        assert_eq!(quoted.predicate.as_str(), "https://ex.org/knows");

        // And the pivot does not choke on a document containing one — it renders
        // the surface form, because a triple term has no identity to key a row by.
        let batch = rdf_to_batch(TTL, "https://ex.org/Reifier", &[]).expect("pivot");
        assert_eq!(batch.num_rows(), 1);
    }

    #[test]
    fn a_base_direction_is_named_where_it_dies() {
        use oxrdf::{BaseDirection, vocab::rdf};

        const TTL: &str = r#"@prefix ex: <https://ex.org/> .
            ex:doc a ex:Doc ; ex:title "مرحبا"@ar--rtl .
        "#;

        // It survives the parser intact, with the disjoint datatype RDF 1.2 gives
        // it: a direction means rdf:dirLangString, its absence rdf:langString.
        let object = TurtleParser::new()
            .for_reader(TTL.as_bytes())
            .map(|t| t.expect("RDF 1.2 Turtle parses").object)
            .find_map(|t| match t {
                Term::Literal(l) => Some(l),
                _ => None,
            })
            .expect("one literal");
        assert_eq!(object.direction(), Some(BaseDirection::Rtl));
        assert_eq!(object.language(), Some("ar"));
        assert_eq!(object.datatype(), rdf::DIR_LANG_STRING);

        // And it dies at the pivot, loudly. The message names the subject, the
        // predicate and the literal, because the alternative is a cell holding
        // "مرحبا" that a host lays out left-to-right.
        let columns = [RdfColumn {
            name: "title".into(),
            predicate: "https://ex.org/title".into(),
            multi: false,
        }];
        let err = rdf_to_batch(TTL, "https://ex.org/Doc", &columns)
            .expect_err("a directional literal is refused, not flattened");
        let msg = err.to_string();
        for expected in [
            "https://ex.org/doc",
            "https://ex.org/title",
            "--rtl",
            "/docs/characteristics/rdf12",
        ] {
            assert!(msg.contains(expected), "{expected:?} missing from {msg:?}");
        }

        // The undirected sibling is untouched — the two are disjoint, so refusing
        // one may not cost the other.
        let plain = TTL.replace("--rtl", "");
        let batch = rdf_to_batch(&plain, "https://ex.org/Doc", &columns).expect("pivot");
        let title = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8 col");
        assert_eq!(title.value(0), "مرحبا");
    }
}
