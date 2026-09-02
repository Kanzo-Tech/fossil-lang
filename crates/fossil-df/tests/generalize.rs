//! Deriving the generalisation, against corpora built to break the derivation.
//!
//! The one to read first is [`a_corpus_that_could_only_be_refused_is_now_released`]:
//! it is the whole reason this module exists. Before it, a declared bound could
//! only ever refuse — nothing in the system could produce a generalised column,
//! so a release passed exactly when the source data happened to be k-anonymous
//! already.
//!
//! Every policy here is built by `fossil_policy::parse` from a real document
//! rather than by filling in a `PrivacyPolicy` literal. That is deliberate: the
//! hierarchy reaches the writer through `fossil:generalization`'s `rightOperand`
//! and nowhere else, so a test that constructed the parsed value directly would
//! be testing the deriver against a vocabulary nobody can actually write.

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, Float32Array, Int32Array, RecordBatch, StringArray, UInt32Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use fossil_df::generalize::{self, Error};
use fossil_df::privacy::{Refusal, verify};
use fossil_df::{GraphArData, VertexTable};
use fossil_graph_schema::{Cardinality, GraphSchema, NodeType, Primitive, Property};
use fossil_policy::PrivacyPolicy;
use fossil_sinks::manifest::{KAnonymity, Privacy};

/// Rows enough for classes worth counting.
const PEOPLE: usize = 60;

/// The two hierarchy documents this file declares — the `numeric` and `prefix`
/// shapes `crates/fossil-kanon/hierarchies/` ships, with buckets and lengths
/// that fit this corpus rather than the shipped values.
///
/// **The shape is the assertion.** The claim `fossil:generalization` makes is
/// that a policy author writes one of these documents into a policy and it
/// works. Two crates that do not depend on each other meet here and nowhere
/// else, which is the meeting `fossil_policy::profile`'s hierarchy-kind test
/// points at.
const AGE_HIERARCHY: &str = r#"{"kind": "numeric", "buckets": [1950, 1955, 1960],
                                "presentation": "enclosing_bucket"}"#;
const POSTCODE_HIERARCHY: &str = r#"{"kind": "prefix", "lengths": [1, 2, 3]}"#;

/// A policy over `Person`, with whatever hierarchies the case declares.
///
/// `generalizations` is a list of `(attribute, hierarchy-json)`; an attribute
/// absent from it is a quasi-identifier published as it is.
fn policy(k: u64, absent: &str, generalizations: &[(&str, &str)]) -> String {
    let mut constraints = vec![
        format!(
            r#"{{"leftOperand": "fossil:anonymityK", "operator": "gteq", "rightOperand": {k}}}"#
        ),
        format!(
            r#"{{"leftOperand": "fossil:absentQuasiIdentifier", "operator": "eq",
                 "rightOperand": "{absent}"}}"#
        ),
    ];
    for attribute in ["birth_year", "postcode"] {
        let hierarchy = generalizations
            .iter()
            .find(|(a, _)| *a == attribute)
            .map(|(_, h)| {
                format!(
                    r#", {{"leftOperand": "fossil:generalization", "operator": "eq",
                       "rightOperand": {h}}}"#
                )
            })
            .unwrap_or_default();
        constraints.push(format!(
            r#"{{"and": [
                 {{"leftOperand": "fossil:attribute", "operator": "eq",
                   "rightOperand": "https://example.org/{attribute}"}},
                 {{"leftOperand": "fossil:classification", "operator": "eq",
                   "rightOperand": "fossil:QuasiIdentifier"}}{hierarchy}]}}"#
        ));
    }
    format!(
        r#"{{
  "@context": ["http://www.w3.org/ns/odrl.jsonld",
               {{"fossil": "https://fossil-lang.org/ns/privacy#",
                 "dpv": "https://w3id.org/dpv#"}}],
  "@type": "Set",
  "uid": "https://example.org/policies/test",
  "profile": "https://fossil-lang.org/ns/privacy/v1",
  "permission": [{{"target": "Person", "action": "use", "constraint": [{}]}}]
}}"#,
        constraints.join(", ")
    )
}

fn parse(document: &str) -> PrivacyPolicy {
    fossil_policy::parse(document).expect("a valid policy document")
}

/// A corpus of `PEOPLE` people, cut into `batches` tiles.
///
/// The two quasi-identifiers are **independent by construction**: `postcode`
/// takes `i % 15` and `birth_year` takes `1950 + i / 15`, so all 60 pairs are
/// distinct and the ungeneralised corpus has 60 classes of one. Every case below
/// starts from a release whose exact `k` is 1, which is the state the deriver
/// exists to get out of.
///
/// `null_postcodes` blanks the postcode of the first `n` rows — the null trap,
/// which derivation is in a position to destroy and must not.
fn corpus(batches: usize, null_postcodes: usize) -> GraphArData {
    let schema = Arc::new(Schema::new(vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("subject", DataType::Utf8, false),
        Field::new("birth_year", DataType::Int32, true),
        Field::new("postcode", DataType::Utf8, true),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ]));
    let per = PEOPLE.div_ceil(batches.max(1));
    let mut out = Vec::new();
    for chunk in (0..PEOPLE).collect::<Vec<_>>().chunks(per.max(1)) {
        // Row identity is the index itself, so a tile boundary moves no value
        // between rows — which is what lets `the_generalisation_is_over_the_release_and_not_the_tile`
        // compare two tilings cell for cell.
        let columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(UInt32Array::from(
                chunk.iter().map(|i| *i as u32).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                chunk
                    .iter()
                    .map(|i| format!("https://example.org/p/{i}"))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                chunk
                    .iter()
                    .map(|i| 1950 + (*i as i32) / 15)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                chunk
                    .iter()
                    .map(|i| {
                        if *i < null_postcodes {
                            None
                        } else {
                            Some(format!("Z{}{}", i % 15 / 5, i % 5))
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float32Array::from(vec![0.0_f32; chunk.len()])),
            Arc::new(Float32Array::from(vec![0.0_f32; chunk.len()])),
            Arc::new(UInt32Array::from(vec![0_u32; chunk.len()])),
        ];
        out.push(RecordBatch::try_new(Arc::clone(&schema), columns).expect("batch"));
    }

    GraphArData {
        privacy: Privacy::Undeclared,
        schema: GraphSchema {
            nodes: vec![NodeType {
                label: "Person".to_string(),
                iri: None,
                properties: vec![
                    Property {
                        name: "birth_year".to_string(),
                        datatype: Primitive::Integer,
                        iri: Some("https://example.org/birth_year".to_string()),
                        cardinality: Cardinality::Single,
                    },
                    Property {
                        name: "postcode".to_string(),
                        datatype: Primitive::String,
                        iri: Some("https://example.org/postcode".to_string()),
                        cardinality: Cardinality::Single,
                    },
                ],
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

/// Derive, then verify — the order and the independence of `run_to_dir`.
fn derive_and_verify(
    policy: &PrivacyPolicy,
    graph: &mut GraphArData,
) -> Result<KAnonymity, String> {
    let derived = generalize::apply(policy, graph).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let sealed = runtime
        .block_on(verify(policy, graph))
        .map_err(|e| e.to_string())?;
    let mut privacy = sealed;
    generalize::record(&derived, &mut privacy);
    match privacy {
        Privacy::KAnonymity(bound) => Ok(bound),
        Privacy::Undeclared => panic!("a policy was declared"),
    }
}

fn column(graph: &GraphArData, name: &str) -> Vec<Option<String>> {
    let table = &graph.vertices[0];
    let mut out = Vec::new();
    for batch in &table.batches {
        let index = batch.schema().index_of(name).expect("a column");
        let array = batch.column(index);
        let strings = array
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("a generalised column is Utf8");
        for row in 0..batch.num_rows() {
            out.push(if strings.is_null(row) {
                None
            } else {
                Some(strings.value(row).to_string())
            });
        }
    }
    out
}

/// **The reason this module exists.**
///
/// The same corpus and the same `k`, twice: once with no hierarchy declared and
/// once with both. Without them the bound can only be refused, because nothing
/// in the system can improve the corpus and its exact `k` is 1. With them it is
/// released — and the number sealed onto the manifest is measured by the
/// verifier, over the written columns, by an aggregate that was handed nothing
/// by the deriver.
#[test]
fn a_corpus_that_could_only_be_refused_is_now_released() {
    let bare = parse(&policy(5, "value", &[]));
    let refusal = derive_and_verify(&bare, &mut corpus(3, 0))
        .expect_err("60 classes of one cannot reach k=5");
    assert!(refusal.contains("k=1"), "{refusal}");

    let declared = parse(&policy(
        5,
        "value",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));
    let bound = derive_and_verify(&declared, &mut corpus(3, 0)).expect("the derivation reaches 5");

    assert_eq!(bound.k, 5);
    assert!(bound.reached >= 5, "{bound:?}");
    // Not a row was dropped to get there. The population the verifier measured
    // is the population the program produced, which is what keeps the scope
    // assertion and the suppression budget meaningful.
    assert_eq!(bound.population, PEOPLE as u64);
    assert_eq!(bound.suppressed, 0);
}

/// The manifest says which columns were generalised and to what.
///
/// `@bucket` for the numeric column because only `enclosing_bucket` is admitted,
/// so every published value is a declared bucket rather than a partition's
/// observed span; `@<coarsest>-<finest>/<declared>` for the levelled one.
#[test]
fn the_manifest_names_every_generalised_column_and_its_levels() {
    let declared = parse(&policy(
        5,
        "value",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));
    let bound = derive_and_verify(&declared, &mut corpus(1, 0)).expect("released");

    let tokens: Vec<&str> = bound.generalization.split(' ').collect();
    assert!(
        tokens.contains(&"Person.birth_year@bucket"),
        "{}",
        bound.generalization
    );
    let postcode = tokens
        .iter()
        .find(|t| t.starts_with("Person.postcode@"))
        .expect(&bound.generalization);
    let levels = postcode.trim_start_matches("Person.postcode@");
    let (reached, declared_levels) = levels.split_once('/').expect(postcode);
    let (coarsest, finest) = reached.split_once('-').expect(postcode);
    assert_eq!(declared_levels, "3", "the hierarchy declares three lengths");
    assert!(coarsest.parse::<usize>().expect(postcode) <= finest.parse::<usize>().expect(postcode));

    // And a run that derives nothing says so with a word rather than a blank —
    // «no hierarchy was declared» has to be readable apart from «written before
    // this field existed», which is what an empty string means.
    let bare = parse(&policy(2, "value", &[]));
    let unbounded = derive_and_verify(&bare, &mut corpus(1, 0));
    if let Ok(bound) = unbounded {
        assert_eq!(bound.generalization, "none");
    }
}

/// **The scope trap, arrived at from the deriver's side.**
///
/// A deriver that partitioned per batch would generalise each tile against its
/// own neighbours, reach `k` inside every tile, and publish a release whose real
/// classes are smaller than any tile's. The corpus is tiles; the equivalence
/// class is the release. So the same rows cut five ways must produce the same
/// generalisation, cell for cell, as the same rows cut once.
#[test]
fn the_generalisation_is_over_the_release_and_not_the_tile() {
    let declared = parse(&policy(
        5,
        "value",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));

    let mut whole = corpus(1, 0);
    let mut tiled = corpus(5, 0);
    let one = derive_and_verify(&declared, &mut whole).expect("released");
    let many = derive_and_verify(&declared, &mut tiled).expect("released");

    assert_eq!(one.reached, many.reached);
    assert_eq!(one.generalization, many.generalization);
    assert_eq!(column(&whole, "postcode"), column(&tiled, "postcode"));
    assert_eq!(column(&whole, "birth_year"), column(&tiled, "birth_year"));

    // And the tiling itself is untouched: the deriver rewrites cells, never the
    // row groups a reader will find.
    assert_eq!(tiled.vertices[0].batches.len(), 5);
    assert_eq!(whole.vertices[0].batches.len(), 1);
}

/// **The null trap, which derivation is in a position to destroy silently.**
///
/// `fossil-kanon` publishes a missing quasi-identifier as `*`, and a `*` written
/// into the corpus is a value as far as every reader downstream is concerned —
/// including the verifier, whose suppression budget counts NULLs and would count
/// none. So an input null is published as a null, and the reading of it stays
/// the policy's to declare.
#[test]
fn a_null_survives_derivation_as_a_null() {
    let declared = parse(&policy(
        5,
        "suppress",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));
    let mut graph = corpus(3, 10);
    let bound = derive_and_verify(&declared, &mut graph);

    let postcodes = column(&graph, "postcode");
    assert_eq!(
        postcodes.iter().take(10).filter(|c| c.is_none()).count(),
        10,
        "ten blanked postcodes must still be blank, and must not have become `*`"
    );
    assert!(
        !postcodes.iter().flatten().any(|c| c == "*"),
        "no cell may be published as the wildcard token"
    );

    // Whatever the bound did, the ten are visible to the budget as suppressed —
    // which is the only reason `suppress` can be charged for at all.
    if let Ok(bound) = bound {
        assert_eq!(bound.suppressed, 10);
        assert_eq!(bound.population, PEOPLE as u64);
    }
}

/// An observed range publishes two real record values, and the writer will not.
#[test]
fn an_observed_range_is_refused_rather_than_published() {
    let observed = r#"{"kind": "numeric", "buckets": [1950, 1955],
                       "presentation": "observed_range"}"#;
    let declared = parse(&policy(
        5,
        "value",
        &[("birth_year", observed), ("postcode", POSTCODE_HIERARCHY)],
    ));
    let err = generalize::apply(&declared, &mut corpus(1, 0)).expect_err("refused");
    assert!(matches!(err, Error::ObservedRangeRefused { .. }), "{err}");
    // The refusal names the column and the fix, and never a birth year.
    let message = err.to_string();
    assert!(message.contains("birth_year"), "{message}");
    assert!(message.contains("enclosing_bucket"), "{message}");
    assert!(!message.contains("1950"), "{message}");
}

/// Generalising some of the tuple optimises for a tuple nobody checks.
///
/// Mondrian reaches `k` over the columns it was handed; the verifier measures
/// the columns that were published. Classes over more columns are never larger,
/// so a partial derivation produces a corpus the deriver believes is fine and
/// the verifier refuses — silently, and for a reason neither message would name.
#[test]
fn generalising_half_the_tuple_is_refused_before_it_can_mislead() {
    let declared = parse(&policy(5, "value", &[("postcode", POSTCODE_HIERARCHY)]));
    let err = generalize::apply(&declared, &mut corpus(1, 0)).expect_err("refused");
    assert!(matches!(err, Error::PartiallyCovered { .. }), "{err}");
    let message = err.to_string();
    assert!(message.contains("birth_year"), "{message}");
}

/// A generalised column changes type in the bytes, so it changes type in the
/// manifest too.
///
/// `GraphArData::manifest` reads each property's datatype off the GRAPH SCHEMA
/// and its row counts off the batches. A `birth_year` that became `Utf8` in the
/// Parquet and stayed `integer` in the schema would be a manifest that lies
/// about its own payload, and a reader that believed it would open nothing.
#[test]
fn the_schema_follows_the_bytes_when_a_column_is_generalised() {
    let declared = parse(&policy(
        5,
        "value",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));
    let mut graph = corpus(1, 0);
    assert_eq!(
        graph.schema.nodes[0].properties[0].datatype,
        Primitive::Integer
    );

    generalize::apply(&declared, &mut graph).expect("derived");

    assert_eq!(
        graph.schema.nodes[0].properties[0].datatype,
        Primitive::String
    );
    assert_eq!(
        graph.vertices[0].batches[0]
            .schema()
            .field_with_name("birth_year")
            .expect("the column")
            .data_type(),
        &DataType::Utf8
    );
}

/// A policy that declares no hierarchy leaves the corpus exactly as it was.
///
/// The behaviour every corpus written before the sixth profile term had, and it
/// stays available: a quasi-identifier already coarse in the source has nothing
/// to generalise, and inventing a hierarchy for it would publish a `*` where a
/// usable value was safe.
#[test]
fn a_policy_with_no_hierarchy_touches_nothing() {
    let bare = parse(&policy(5, "value", &[]));
    let mut graph = corpus(2, 0);
    let before = column(&graph, "postcode");

    let derived = generalize::apply(&bare, &mut graph).expect("nothing to do");

    assert_eq!(derived.render(), "none");
    assert_eq!(column(&graph, "postcode"), before);
    assert_eq!(
        graph.schema.nodes[0].properties[0].datatype,
        Primitive::Integer
    );
}

/// The verifier is not told what the deriver did, and the type system is where
/// that is enforced rather than a comment.
///
/// `verify` takes a policy and a corpus. There is no call shape in which a
/// derivation's achieved `k`, class count or column list can reach it — the
/// only thing [`generalize::apply`] returns is the manifest string, and
/// `Refusal` is reachable from a derived corpus exactly as it is from any other.
#[test]
fn a_derived_corpus_is_refused_by_the_same_check_as_any_other() {
    // A `k` larger than the release. No generalisation reaches it, because the
    // whole table is one class of 60 at the top of every hierarchy and 60 is
    // still not 100 — which is the one bound derivation genuinely cannot buy.
    let declared = parse(&policy(
        100,
        "value",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));
    let mut graph = corpus(2, 0);
    // The derivation itself does not fail. It reaches what it can and hands the
    // corpus on; deciding whether that was enough is not its call to make.
    let derived = generalize::apply(&declared, &mut graph).expect("derived");
    assert_ne!(derived.render(), "none");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let refusal = runtime
        .block_on(verify(&declared, &graph))
        .expect_err("the verifier measures the bytes, not the deriver's opinion");
    assert!(
        matches!(refusal, Refusal::BoundNotReached { .. }),
        "{refusal}"
    );
}

/// **A `k` this corpus can only reach by publishing nothing, and the manifest
/// says which.**
///
/// This is the hazard the `generalization` field earns its place on. Mondrian
/// reaches any `k` up to the row count, because the top of every hierarchy is
/// `*` and one class of everybody satisfies any bound it is large enough for.
/// So a demanding `k` does not produce a refusal here — it produces a **valid,
/// useless release**: a table of asterisks that clears its bound and carries no
/// information, and whose `reached: 60` looks identical to a real one.
///
/// Nothing in `k`, `reached`, `population` or `suppressed` distinguishes it.
/// `generalization` does, and it is the only field that does: a coarsest and a
/// finest that are the same number, well short of the levels the hierarchy
/// declared, is a column every row of which publishes one value. A recipient
/// reads that off the manifest rather than discovering it in the Parquet.
///
/// (The number is `1` and not `0` here because every postcode in this fixture
/// begins with the same character, so the first declared prefix length already
/// flattens the column — the top of a hierarchy is not the only place a column
/// can go flat, which is exactly why the field reports the level reached rather
/// than a flattened/not-flattened flag.)
///
/// The writer does not refuse this. Utility is not a property the verifier can
/// measure and a threshold on it would be this file inventing a policy
/// parameter; what it can do is refuse to let the flattening be invisible.
#[test]
fn a_release_generalised_to_the_top_is_valid_and_says_so() {
    let declared = parse(&policy(
        40,
        "value",
        &[
            ("birth_year", AGE_HIERARCHY),
            ("postcode", POSTCODE_HIERARCHY),
        ],
    ));
    let mut graph = corpus(2, 0);
    let bound = derive_and_verify(&declared, &mut graph).expect("one class of sixty clears 40");

    assert_eq!(bound.reached, 60);

    // One level, and it is not the finest the hierarchy offers: the column is
    // flat, and the manifest is where that is readable.
    let token = bound
        .generalization
        .split(' ')
        .find(|t| t.starts_with("Person.postcode@"))
        .expect(&bound.generalization)
        .trim_start_matches("Person.postcode@");
    let (reached_levels, declared_levels) = token.split_once('/').expect(token);
    let (coarsest, finest) = reached_levels.split_once('-').expect(token);
    assert_eq!(coarsest, finest, "a flat column sits at one level: {token}");
    assert!(
        finest.parse::<usize>().expect(token) < declared_levels.parse::<usize>().expect(token),
        "and short of what the hierarchy declared: {token}"
    );

    // And the bytes agree with the manifest: the whole column is one value, so
    // the release satisfies its bound by carrying no postcode information at all.
    let postcodes = column(&graph, "postcode");
    let distinct: std::collections::BTreeSet<_> = postcodes.iter().flatten().collect();
    assert_eq!(distinct.len(), 1, "{distinct:?}");
}
