//! The write-time privacy check, against corpora built to break it.
//!
//! Every case here is one of the three traps in `src/privacy.rs`'s header, or
//! one of the refusals. The one worth reading first is
//! [`the_two_null_readings_disagree_on_one_file`]: it is the measurement that
//! justifies the whole parameter, and without it "both extremes are wrong"
//! would be a sentence.

use std::sync::Arc;

use datafusion::arrow::array::{Float32Array, RecordBatch, StringArray, UInt32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use fossil_df::privacy::{Refusal, verify};
use fossil_df::{GraphArData, VertexTable};
use fossil_graph_schema::{Cardinality, GraphSchema, NodeType, Primitive, Property};
use fossil_policy::{AbsentQuasiIdentifier, Classification, PrivacyPolicy, ShapeRule};
use fossil_sinks::manifest::Privacy;

/// The corpus column shape a fossil writer produces, plus whatever quasi-
/// identifiers the case needs. `postcode` is nullable on purpose: every null
/// case below moves values in and out of it.
fn schema(props: &[&str]) -> Arc<Schema> {
    let mut fields = vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
    ];
    for p in props {
        fields.push(Field::new(*p, DataType::Utf8, true));
    }
    fields.push(Field::new("x", DataType::Float32, false));
    fields.push(Field::new("y", DataType::Float32, false));
    fields.push(Field::new("cluster_id", DataType::UInt32, false));
    Arc::new(Schema::new(fields))
}

/// A corpus of one vertex type, cut into `batches` batches.
///
/// **The cut is the point of the parameter.** A corpus is tiles, so the check
/// has to give the same answer however the rows are divided, and
/// [`the_class_is_the_release_and_not_the_tile`] is what makes that a test
/// rather than a hope.
fn corpus(props: &[&str], rows: &[Vec<Option<&str>>], batches: usize) -> GraphArData {
    let schema = schema(props);
    let per = rows.len().div_ceil(batches.max(1));
    let mut out = Vec::new();
    for (b, chunk) in rows.chunks(per.max(1)).enumerate() {
        let base = (b * per) as u32;
        let mut columns: Vec<Arc<dyn datafusion::arrow::array::Array>> = vec![
            Arc::new(UInt32Array::from(
                (0..chunk.len())
                    .map(|i| base + i as u32)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                (0..chunk.len())
                    .map(|i| format!("https://example.org/p/{}", base as usize + i))
                    .collect::<Vec<_>>(),
            )),
        ];
        for p in 0..props.len() {
            columns.push(Arc::new(StringArray::from(
                chunk.iter().map(|r| r[p]).collect::<Vec<_>>(),
            )));
        }
        columns.push(Arc::new(Float32Array::from(vec![0.0_f32; chunk.len()])));
        columns.push(Arc::new(Float32Array::from(vec![0.0_f32; chunk.len()])));
        columns.push(Arc::new(UInt32Array::from(vec![0_u32; chunk.len()])));
        out.push(RecordBatch::try_new(schema.clone(), columns).expect("batch"));
    }

    GraphArData {
        // Undeclared until measured, which is what `verify` returns rather than
        // mutates: the value it is handed is one no bound has touched.
        privacy: Privacy::Undeclared,
        schema: GraphSchema {
            nodes: vec![NodeType {
                label: "Person".to_string(),
                iri: None,
                properties: props
                    .iter()
                    .map(|p| Property {
                        name: (*p).to_string(),
                        datatype: Primitive::String,
                        iri: Some(format!("https://example.org/{p}")),
                        cardinality: Cardinality::Single,
                    })
                    .collect(),
            }],
            edges: vec![],
        },
        vertices: vec![VertexTable {
            label: "Person".to_string(),
            batches: out,
        }],
        edges: vec![],
    }
}

fn policy(k: u64, absent: AbsentQuasiIdentifier, rule: ShapeRule) -> PrivacyPolicy {
    PrivacyPolicy {
        uid: "https://example.org/policies/test".to_string(),
        profile: "https://fossil-lang.org/ns/privacy/v1".to_string(),
        k,
        absent_quasi_identifier: absent,
        suppression_budget_ppm: 20_000,
        shapes: vec![rule],
    }
}

fn person(quasi: &[&str]) -> ShapeRule {
    ShapeRule {
        shape: "Person".to_string(),
        classification: quasi
            .iter()
            .map(|q| ((*q).to_string(), Classification::QuasiIdentifier))
            .collect(),
        quasi_identifiers: quasi.iter().map(|q| (*q).to_string()).collect(),
        prohibited: vec![],
        // No hierarchy. Every test in this file measures a corpus somebody
        // handed the verifier, which is the whole point of the file: the
        // verifier does not derive, and its arithmetic must be assertable
        // without anything having been derived first. `tests/generalize.rs`
        // is where a declared hierarchy is exercised.
        generalizations: vec![],
    }
}

fn bound(privacy: &Privacy) -> &fossil_sinks::manifest::KAnonymity {
    match privacy {
        Privacy::KAnonymity(b) => b,
        Privacy::Undeclared => panic!("expected a bound"),
    }
}

/// `n` copies of one row.
fn repeat<'a>(row: &[Option<&'a str>], n: usize) -> Vec<Vec<Option<&'a str>>> {
    (0..n).map(|_| row.to_vec()).collect()
}

#[tokio::test]
async fn a_release_that_clears_the_bar_is_sealed_with_what_it_reached() {
    // Three classes of six. k=5 is asked for and 6 is what there is, and the
    // manifest carries BOTH — the margin is information a recipient acts on.
    let mut rows = repeat(&[Some("1980"), Some("SW1")], 6);
    rows.extend(repeat(&[Some("1981"), Some("SW1")], 6));
    rows.extend(repeat(&[Some("1982"), Some("SW2")], 6));
    let graph = corpus(&["birth_year", "postcode"], &rows, 1);

    let sealed = verify(
        &policy(
            5,
            AbsentQuasiIdentifier::Value,
            person(&["birth_year", "postcode"]),
        ),
        &graph,
    )
    .await
    .expect("the bound holds");

    let b = bound(&sealed);
    assert_eq!(b.k, 5);
    assert_eq!(b.reached, 6);
    assert_eq!(b.population, 18);
    assert_eq!(b.suppressed, 0);
    // Sorted and qualified, so the string is a function of the corpus rather
    // than of the order the policy listed things in.
    assert_eq!(b.quasi_identifiers, "Person.birth_year Person.postcode");
    assert_eq!(b.policy, "https://example.org/policies/test");
}

#[tokio::test]
async fn a_release_that_does_not_is_refused_and_says_how_far_it_got() {
    // One class of six and two singletons. `reached` alone would be 1 either
    // way; what tells a producer whether this is a rounding problem or a broken
    // release is that there are TWO classes below and TWO records in them.
    let mut rows = repeat(&[Some("1980"), Some("SW1")], 6);
    rows.push(vec![Some("1999"), Some("EC4")]);
    rows.push(vec![Some("1998"), Some("EC4")]);
    let graph = corpus(&["birth_year", "postcode"], &rows, 1);

    let err = verify(
        &policy(
            5,
            AbsentQuasiIdentifier::Value,
            person(&["birth_year", "postcode"]),
        ),
        &graph,
    )
    .await
    .expect_err("the bound does not hold");

    match err {
        Refusal::BoundNotReached {
            reached,
            below,
            at_risk,
            population,
            ..
        } => {
            assert_eq!((reached, below, at_risk, population), (1, 2, 2, 8));
        }
        other => panic!("{other:?}"),
    }
    // And it named no value. The whole refusal is counts and column names.
    let rendered = format!(
        "{}",
        verify(
            &policy(
                5,
                AbsentQuasiIdentifier::Value,
                person(&["birth_year", "postcode"])
            ),
            &graph
        )
        .await
        .unwrap_err()
    );
    assert!(!rendered.contains("1999"), "{rendered}");
    assert!(!rendered.contains("EC4"), "{rendered}");
}

/// **The measurement the declared parameter exists for.**
///
/// One file, one quasi-identifier tuple, three readings, three different
/// answers. Six records share `(1980, SW1)`; four more carry `1980` with an
/// absent postcode.
///
/// - `value` — the four absent postcodes are a category of their own, so the
///   classes are 6 and 4 and `k` is **4**.
/// - `wildcard` — each of the four could be any postcode, so it joins the class
///   of six; and the six borrow the four back. `k` is **10**.
/// - `suppress` — the four are not certified at all: they are charged to the
///   budget, and the certified population is the six. `k` is **6**.
///
/// Nothing about the bytes changed between those three numbers. That is the
/// entire argument for the parameter, and it is why sdcMicro exposes the same
/// choice numerically rather than picking.
#[tokio::test]
async fn the_two_null_readings_disagree_on_one_file() {
    let mut rows = repeat(&[Some("1980"), Some("SW1")], 6);
    rows.extend(repeat(&[Some("1980"), None], 4));
    let graph = corpus(&["birth_year", "postcode"], &rows, 1);
    let quasi = ["birth_year", "postcode"];

    let value = verify(
        &policy(1, AbsentQuasiIdentifier::Value, person(&quasi)),
        &graph,
    )
    .await
    .expect("value");
    assert_eq!(bound(&value).reached, 4);
    assert_eq!(bound(&value).suppressed, 0);

    let wildcard = verify(
        &policy(1, AbsentQuasiIdentifier::Wildcard, person(&quasi)),
        &graph,
    )
    .await
    .expect("wildcard");
    assert_eq!(bound(&wildcard).reached, 10);

    // A budget wide enough to let this case through, because the budget is a
    // SEPARATE refusal and `suppression_past_the_budget_is_refused` is where it
    // is tested. Four of ten is 400,000 ppm and the default here is 20,000, so
    // without this the reading under test never gets reported.
    let mut wide = policy(1, AbsentQuasiIdentifier::Suppress, person(&quasi));
    wide.suppression_budget_ppm = 500_000;
    let suppress = verify(&wide, &graph).await.expect("suppress");
    assert_eq!(bound(&suppress).reached, 6);
    assert_eq!(bound(&suppress).suppressed, 4);
    // And the population is still the release. Suppression takes records out of
    // CERTIFICATION, not out of the count they are certified against — a rate
    // measured against the certified subset would fall as more was suppressed.
    assert_eq!(bound(&suppress).population, 10);

    // The ordering the docblock claims: `value` is the conservative end, so a
    // corpus passing under it passes under `wildcard`, always.
    assert!(bound(&value).reached <= bound(&wildcard).reached);
}

/// A corpus is tiles, and a per-tile aggregation is the easiest wrong answer
/// available here — every class it finds is a subset of a real one, so it
/// reports a `k` that is too small and looks conservative while being
/// unverified.
///
/// Same rows, cut seventeen ways, same answer. If the aggregate ever ran per
/// batch this would report 1.
#[tokio::test]
async fn the_class_is_the_release_and_not_the_tile() {
    let mut rows = repeat(&[Some("1980"), Some("SW1")], 34);
    rows.extend(repeat(&[Some("1981"), Some("SW2")], 34));

    let whole = verify(
        &policy(
            5,
            AbsentQuasiIdentifier::Value,
            person(&["birth_year", "postcode"]),
        ),
        &corpus(&["birth_year", "postcode"], &rows, 1),
    )
    .await
    .expect("one batch");

    let tiled = verify(
        &policy(
            5,
            AbsentQuasiIdentifier::Value,
            person(&["birth_year", "postcode"]),
        ),
        &corpus(&["birth_year", "postcode"], &rows, 17),
    )
    .await
    .expect("seventeen batches");

    assert_eq!(bound(&whole).reached, 34);
    assert_eq!(bound(&tiled).reached, 34);
    assert_eq!(bound(&whole), bound(&tiled));
}

#[tokio::test]
async fn suppression_past_the_budget_is_refused() {
    // 4 of 10 absent is 400,000 ppm against a 20,000 ppm budget. The bound is
    // REACHED — the certified six clear k=5 — and the release is still refused,
    // which is the whole point of counting suppression separately from k.
    let mut rows = repeat(&[Some("1980"), Some("SW1")], 6);
    rows.extend(repeat(&[Some("1980"), None], 4));
    let graph = corpus(&["birth_year", "postcode"], &rows, 1);

    let err = verify(
        &policy(
            5,
            AbsentQuasiIdentifier::Suppress,
            person(&["birth_year", "postcode"]),
        ),
        &graph,
    )
    .await
    .expect_err("over budget");

    match err {
        Refusal::BudgetExceeded {
            suppressed,
            population,
            spent,
            budget,
            ..
        } => assert_eq!(
            (suppressed, population, spent, budget),
            (4, 10, 400_000, 20_000)
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_type_the_policy_says_nothing_about_is_refused_rather_than_exempt() {
    let graph = corpus(&["birth_year"], &repeat(&[Some("1980")], 8), 1);
    let mut p = policy(5, AbsentQuasiIdentifier::Value, person(&["birth_year"]));
    p.shapes[0].shape = "Order".to_string();

    match verify(&p, &graph).await.expect_err("not covered") {
        Refusal::ShapeNotCovered { shape } => assert_eq!(shape, "Person"),
        other => panic!("{other:?}"),
    }
}

/// The escape from the rule above is one line, and it has to work: a type
/// carrying nothing to protect gets a rule with an empty set.
///
/// An empty quasi-identifier tuple is not a special case in the arithmetic —
/// it is one equivalence class holding everybody, so `reached` is the
/// population. That falls out of `GROUP BY ()` and is asserted here because it
/// is the kind of thing a later refactor adds a special case for.
#[tokio::test]
async fn a_type_with_nothing_to_protect_declares_it_and_reaches_its_own_population() {
    let graph = corpus(&["colour"], &repeat(&[Some("red")], 8), 1);
    let sealed = verify(
        &policy(5, AbsentQuasiIdentifier::Value, person(&[])),
        &graph,
    )
    .await
    .expect("nothing to protect");
    assert_eq!(bound(&sealed).reached, 8);
    assert_eq!(bound(&sealed).population, 8);
    assert_eq!(bound(&sealed).quasi_identifiers, "");
}

#[tokio::test]
async fn a_published_direct_identifier_is_refused_whatever_k_says() {
    // Eight identical quasi-identifier tuples: k is 8 and the release is still
    // refused, because a column naming the person makes the tuple irrelevant.
    let rows = repeat(&[Some("1980"), Some("a@example.org")], 8);
    let graph = corpus(&["birth_year", "email"], &rows, 1);
    let mut p = policy(5, AbsentQuasiIdentifier::Value, person(&["birth_year"]));
    p.shapes[0]
        .classification
        .push(("email".to_string(), Classification::DirectIdentifier));

    match verify(&p, &graph).await.expect_err("direct identifier") {
        Refusal::DirectIdentifierPublished { column, .. } => assert_eq!(column, "email"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_prohibited_column_is_refused_by_being_present() {
    let rows = repeat(&[Some("1980"), Some("x")], 8);
    let graph = corpus(&["birth_year", "diagnosis"], &rows, 1);
    let mut p = policy(5, AbsentQuasiIdentifier::Value, person(&["birth_year"]));
    // By IRI, which is how a policy written against a vocabulary names it —
    // the column is `diagnosis` and the policy never says that word.
    p.shapes[0]
        .prohibited
        .push("https://example.org/diagnosis".to_string());

    match verify(&p, &graph).await.expect_err("prohibited") {
        Refusal::ProhibitedPublished { column, .. } => assert_eq!(column, "diagnosis"),
        other => panic!("{other:?}"),
    }
}

/// A policy may name a quasi-identifier this release does not publish. The
/// tuple the bound is over is the one that EXISTS, and the manifest publishes
/// that list rather than the policy's — fewer columns is a coarser and stronger
/// claim, but it is a different claim, and a recipient re-deriving `k` has to
/// be told which columns to re-derive it over.
#[tokio::test]
async fn the_tuple_published_is_the_one_the_corpus_carries() {
    let graph = corpus(&["birth_year"], &repeat(&[Some("1980")], 8), 1);
    let sealed = verify(
        &policy(
            5,
            AbsentQuasiIdentifier::Value,
            person(&["birth_year", "postcode", "sex"]),
        ),
        &graph,
    )
    .await
    .expect("the published subset");
    assert_eq!(bound(&sealed).quasi_identifiers, "Person.birth_year");
}
