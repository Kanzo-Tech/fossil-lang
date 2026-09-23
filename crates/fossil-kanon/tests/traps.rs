//! One test per trap, each named after the thing that goes wrong.
//!
//! The property test in `invariant.rs` proves the contract holds. These prove the crate reaches it
//! the *right way* — a table can satisfy k by generalising everything to `*`, and every one of
//! these would still pass if it did. So they assert the shape of the output, not only its safety.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, Date32Array, Float64Array, Int32Array, StringArray};
use fossil_kanon::hierarchy::{Date, DateLevel, Hierarchy, Numeric, NumericPresentation, Prefix};
use fossil_kanon::{
    Anonymized, ColumnLevels, Config, Error, NullPolicy, QuasiIdentifier, Warning, anonymize,
};

fn ints(v: Vec<Option<i32>>) -> ArrayRef {
    Arc::new(Int32Array::from(v))
}

fn strs(v: Vec<Option<&str>>) -> ArrayRef {
    Arc::new(StringArray::from(v))
}

fn numeric_qi(
    values: ArrayRef,
    presentation: NumericPresentation,
    buckets: Vec<f64>,
) -> QuasiIdentifier {
    QuasiIdentifier {
        name: "age".into(),
        values,
        hierarchy: Hierarchy::Numeric(Numeric {
            buckets,
            presentation,
        }),
    }
}

fn observed(values: ArrayRef) -> QuasiIdentifier {
    numeric_qi(values, NumericPresentation::ObservedRange, vec![])
}

fn run(qis: &[QuasiIdentifier], k: usize, nulls: NullPolicy) -> Anonymized {
    anonymize(qis, &Config { k, nulls }).expect("a well-formed run")
}

/// Every published value of column `d`, in row order, for the released rows.
fn published(out: &Anonymized, d: usize) -> Vec<String> {
    let col = out.columns[d]
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    (0..col.len())
        .filter(|&i| !col.is_null(i))
        .map(|i| col.value(i).to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Trap 1. The median value has k or more duplicates.
// ---------------------------------------------------------------------------------------------

/// Four records share the median value and one does not. Relaxed partitioning would deal the tied
/// records across both sides to balance them, producing a right-hand class of one and reporting
/// k = 2. Strict refuses the cut and publishes one class of five.
#[test]
fn a_median_with_k_duplicates_refuses_the_cut_rather_than_splitting_the_ties() {
    let out = run(
        &[observed(ints(vec![
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(2),
        ]))],
        2,
        NullPolicy::Wildcard,
    );
    assert_eq!(out.assessment.classes.len(), 1);
    assert_eq!(out.report.achieved_k, Some(5));
    assert_eq!(out.report.released_rows, 5);
    assert_eq!(out.report.suppressed_rows, 0);
    assert_eq!(published(&out, 0), vec!["[1, 2]"; 5]);
}

/// The same shape one step further: the minority side is a single record and the cut that would
/// isolate it is refused however the tie is arranged.
#[test]
fn a_lone_outlier_does_not_become_a_class_of_its_own() {
    let out = run(
        &[observed(ints(vec![
            Some(5),
            Some(5),
            Some(5),
            Some(5),
            Some(5),
            Some(9),
        ]))],
        2,
        NullPolicy::Wildcard,
    );
    assert_eq!(out.assessment.classes.len(), 1);
    assert_eq!(out.report.achieved_k, Some(6));
}

#[test]
fn a_column_of_one_repeated_value_is_one_class() {
    let out = run(&[observed(ints(vec![Some(7); 9]))], 3, NullPolicy::Wildcard);
    assert_eq!(out.assessment.classes.len(), 1);
    assert_eq!(published(&out, 0), vec!["7"; 9]);
}

// ---------------------------------------------------------------------------------------------
// Trap 2. Emitted [min, max] intervals leak real record endpoints.
// ---------------------------------------------------------------------------------------------

/// Under `EnclosingBucket` every published span is built from two DECLARED bucket edges, so no
/// endpoint is a value any record holds. The assertion is exhaustive rather than illustrative: the
/// full set of strings the declared edges can produce is enumerated, and the output must be a
/// subset of it.
#[test]
fn an_enclosing_bucket_publishes_no_record_endpoint() {
    let edges = [18.0_f64, 30.0, 45.0, 65.0];
    let out = run(
        &[numeric_qi(
            ints(vec![
                Some(21),
                Some(24),
                Some(29),
                Some(33),
                Some(41),
                Some(47),
            ]),
            NumericPresentation::EnclosingBucket,
            edges.to_vec(),
        )],
        3,
        NullPolicy::Wildcard,
    );

    let mut legal: Vec<String> = Vec::new();
    for a in edges {
        for b in edges {
            if a < b {
                legal.push(format!("[{a:.0}, {b:.0})"));
            }
        }
        legal.push(format!("[{a:.0}, inf)"));
        legal.push(format!("(-inf, {a:.0})"));
    }
    for v in published(&out, 0) {
        assert!(
            legal.contains(&v),
            "{v} is not a span of the declared edges"
        );
    }
    assert!(out.report.released_rows == 6);
    // And nothing warns: the buckets cover the data.
    assert!(!out.report.warnings.iter().any(|w| matches!(
        w,
        Warning::BucketsDoNotCover { .. } | Warning::ObservedRangeEndpoints { .. }
    )));
}

/// Choosing to publish real endpoints warns, every run. The warning is the whole of the mitigation.
#[test]
fn observed_range_warns_that_the_endpoints_are_real_records() {
    let out = run(
        &[observed(ints(vec![Some(29), Some(44), Some(61), Some(12)]))],
        2,
        NullPolicy::Wildcard,
    );
    assert!(
        out.report
            .warnings
            .contains(&Warning::ObservedRangeEndpoints {
                column: "age".into()
            })
    );
    // And the endpoints really are record values — which is what the warning says.
    assert!(published(&out, 0).iter().any(|v| v.contains("61")));
}

#[test]
fn a_value_outside_the_declared_buckets_publishes_an_open_span_and_warns() {
    let out = run(
        &[numeric_qi(
            ints(vec![Some(5), Some(6), Some(90), Some(95)]),
            NumericPresentation::EnclosingBucket,
            vec![18.0, 65.0],
        )],
        2,
        NullPolicy::Wildcard,
    );
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::BucketsDoNotCover { .. }))
    );
    for v in published(&out, 0) {
        assert!(v.starts_with("(-inf") || v.ends_with("inf)"), "{v}");
    }
}

// ---------------------------------------------------------------------------------------------
// Trap 3. Categorical quasi-identifiers need hierarchy-aware splits.
// ---------------------------------------------------------------------------------------------

/// A lexicographic median cut over these six strings would publish a class as a RANGE of strings —
/// something like `[AA1, AB2]` — which is not a postcode district and not a node of any hierarchy.
/// Every published value here is a declared prefix.
#[test]
fn a_categorical_split_follows_the_hierarchy_and_not_the_alphabet() {
    let out = run(
        &[QuasiIdentifier {
            name: "postcode".into(),
            values: strs(vec![
                Some("AA1"),
                Some("AA2"),
                Some("AB1"),
                Some("AB2"),
                Some("BA1"),
                Some("BA2"),
            ]),
            hierarchy: Hierarchy::Prefix(Prefix {
                lengths: vec![1, 2, 3],
            }),
        }],
        2,
        NullPolicy::Wildcard,
    );
    for v in published(&out, 0) {
        assert!(
            v.ends_with('*') && v.trim_end_matches('*').chars().all(char::is_alphanumeric),
            "{v} is not a declared prefix node"
        );
    }
    // The hierarchy separated AA from AB, which a two-way cut on a single dimension could not have
    // done at the same time as separating A from B.
    let vals = published(&out, 0);
    assert!(vals.contains(&"AA*".to_owned()));
    assert!(vals.contains(&"AB*".to_owned()));
}

#[test]
fn a_date_generalises_towards_the_year_and_not_towards_an_interval_of_days() {
    let days: ArrayRef = Arc::new(Date32Array::from(vec![
        // 2019-01-01, 2019-01-02, 2019-06-01, 2020-03-01, 2020-03-02, 2020-09-09
        17_897, 17_898, 18_048, 18_322, 18_323, 18_514,
    ]));
    let out = run(
        &[QuasiIdentifier {
            name: "born".into(),
            values: days,
            hierarchy: Hierarchy::Date(Date {
                levels: vec![DateLevel::Year, DateLevel::Month, DateLevel::Day],
            }),
        }],
        3,
        NullPolicy::Wildcard,
    );
    let mut vals = published(&out, 0);
    vals.sort();
    vals.dedup();
    assert_eq!(vals, vec!["2019".to_owned(), "2020".to_owned()]);
}

// ---------------------------------------------------------------------------------------------
// Trap 4. Null semantics.
// ---------------------------------------------------------------------------------------------

/// The record with no age is not a class of one. It publishes `*`, which is compatible with every
/// other published value, so its anonymity set is the whole released table — while the classes it
/// is compatible with grow by one rather than losing it.
#[test]
fn a_missing_quasi_identifier_is_a_wildcard_and_not_a_class_of_its_own() {
    let out = run(
        &[observed(ints(vec![
            Some(10),
            Some(10),
            Some(10),
            Some(50),
            Some(50),
            Some(50),
            None,
        ]))],
        3,
        NullPolicy::Wildcard,
    );
    let wild = out
        .assessment
        .classes
        .iter()
        .find(|c| c.values == vec!["*".to_owned()])
        .expect("the missing value publishes a wildcard class");
    assert_eq!(wild.exact_rows, 1);
    assert_eq!(wild.anonymity_set, 7, "a wildcard matches the whole table");

    for c in &out.assessment.classes {
        if c.values != vec!["*".to_owned()] {
            assert_eq!(c.exact_rows, 3);
            assert_eq!(
                c.anonymity_set, 4,
                "the wildcard enlarges it rather than leaving"
            );
        }
    }
    assert_eq!(out.report.suppressed_rows, 0);
}

#[test]
fn suppress_withholds_the_records_with_missing_values_and_counts_them() {
    let out = run(
        &[observed(ints(vec![
            Some(10),
            Some(10),
            Some(10),
            Some(50),
            Some(50),
            Some(50),
            None,
        ]))],
        3,
        NullPolicy::Suppress,
    );
    assert_eq!(out.report.suppressed_by_null_policy, 1);
    assert_eq!(out.report.suppressed_rows, 1);
    assert_eq!(out.report.released_rows, 6);
    assert!(!out.released[6]);
    // No wildcard reaches the output, so exact classes and anonymity sets coincide.
    assert_eq!(out.report.achieved_k, out.report.smallest_exact_class);
}

// ---------------------------------------------------------------------------------------------
// Trap 5. High dimensionality (Aggarwal, 2005).
// ---------------------------------------------------------------------------------------------

#[test]
fn eleven_quasi_identifiers_warn_rather_than_returning_rubbish_quietly() {
    let qis: Vec<QuasiIdentifier> = (0..11)
        .map(|i| QuasiIdentifier {
            name: format!("q{i}"),
            values: ints((0..12).map(Some).collect()),
            hierarchy: Hierarchy::Numeric(Numeric {
                buckets: vec![],
                presentation: NumericPresentation::ObservedRange,
            }),
        })
        .collect();
    let out = run(&qis, 3, NullPolicy::Wildcard);
    assert!(out.report.warnings.contains(&Warning::HighDimensionality {
        quasi_identifiers: 11,
        threshold: fossil_kanon::HIGH_DIMENSIONALITY_THRESHOLD,
    }));
    // Still k-anonymous, which is the point: the warning is about utility, not safety.
    assert!(out.assessment.satisfies_k);
}

// ---------------------------------------------------------------------------------------------
// Trap 6. Local recoding does not produce a hierarchy level.
// ---------------------------------------------------------------------------------------------

/// The report cannot be asked «what level did the age column reach», because there is no answer.
/// A levelled column answers; a numeric one reports spans.
#[test]
fn a_numeric_column_reports_spans_and_a_levelled_one_reports_levels() {
    let out = run(
        &[
            observed(ints((0..12).map(Some).collect())),
            QuasiIdentifier {
                name: "postcode".into(),
                values: strs(vec![Some("AA1"); 12]),
                hierarchy: Hierarchy::Prefix(Prefix {
                    lengths: vec![1, 2, 3],
                }),
            },
        ],
        3,
        NullPolicy::Wildcard,
    );
    assert!(matches!(
        out.report.columns[0].levels,
        ColumnLevels::Spans { .. }
    ));
    match out.report.columns[1].levels {
        ColumnLevels::Levels {
            finest, declared, ..
        } => {
            assert_eq!(declared, 3);
            assert_eq!(
                finest, 3,
                "one distinct postcode refines to the leaf freely"
            );
        }
        ColumnLevels::Spans { .. } => panic!("a prefix hierarchy has levels"),
    }
}

// ---------------------------------------------------------------------------------------------
// The degenerate inputs.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_table_smaller_than_k_releases_nothing_and_says_which_reason_it_was() {
    let out = run(
        &[observed(ints(vec![Some(1), Some(2)]))],
        5,
        NullPolicy::Wildcard,
    );
    assert_eq!(out.report.released_rows, 0);
    assert_eq!(out.report.suppressed_below_k, 2);
    assert_eq!(out.report.suppressed_by_null_policy, 0);
    assert_eq!(out.report.achieved_k, None);
    assert!(
        out.report
            .warnings
            .contains(&Warning::EverythingSuppressed {
                input_rows: 2,
                k: 5
            })
    );
    assert!(out.columns[0].is_null(0) && out.columns[0].is_null(1));
}

#[test]
fn an_empty_table_is_a_run_and_not_an_error() {
    let out = run(&[observed(ints(vec![]))], 3, NullPolicy::Wildcard);
    assert_eq!(out.report.input_rows, 0);
    assert_eq!(out.report.released_rows, 0);
    assert!(out.assessment.satisfies_k, "vacuously");
}

#[test]
fn k_below_two_is_refused_because_it_is_the_identity() {
    assert!(matches!(
        anonymize(
            &[observed(ints(vec![Some(1)]))],
            &Config {
                k: 1,
                nulls: NullPolicy::Wildcard
            }
        ),
        Err(Error::KTooSmall { k: 1 })
    ));
}

#[test]
fn a_nan_is_refused_rather_than_read_as_missing() {
    let vals: ArrayRef = Arc::new(Float64Array::from(vec![1.0, f64::NAN, 3.0]));
    assert!(matches!(
        anonymize(
            &[observed(vals)],
            &Config {
                k: 2,
                nulls: NullPolicy::Wildcard
            }
        ),
        Err(Error::NotANumber { row: 1, .. })
    ));
}

#[test]
fn a_string_column_under_a_numeric_hierarchy_names_the_mismatch() {
    let err = anonymize(
        &[observed(strs(vec![Some("x"), Some("y")]))],
        &Config {
            k: 2,
            nulls: NullPolicy::Wildcard,
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::UnsupportedType { hierarchy, .. } if hierarchy == "numeric"));
    assert!(
        err.to_string().contains("age"),
        "the error names the column"
    );
}

// ---------------------------------------------------------------------------------------------
// The whole report, exactly.
// ---------------------------------------------------------------------------------------------

/// A realistic three-column release, snapshotted whole. Six `assert_eq!`s over a dozen fields pass
/// while the seventh field changes silently; this does not.
#[test]
fn the_report_of_a_realistic_release() {
    let ages = ints(vec![
        Some(23),
        Some(27),
        Some(31),
        Some(34),
        Some(38),
        Some(42),
        Some(51),
        Some(55),
        Some(58),
        Some(62),
        Some(67),
        Some(71),
    ]);
    let posts = strs(vec![
        Some("SW1A 1AA"),
        Some("SW1A 2BB"),
        Some("SW1P 3CC"),
        Some("SW1P 4DD"),
        Some("N1 5EE"),
        Some("N1 6FF"),
        Some("N7 7GG"),
        Some("N7 8HH"),
        Some("EC1A 9JJ"),
        Some("EC1A 1KK"),
        Some("EC2V 2LL"),
        Some("EC2V 3MM"),
    ]);
    let out = run(
        &[
            QuasiIdentifier {
                name: "age".into(),
                values: ages,
                hierarchy: Hierarchy::Numeric(Numeric::bucketed(vec![
                    0.0, 18.0, 30.0, 45.0, 65.0, 80.0,
                ])),
            },
            QuasiIdentifier {
                name: "postcode".into(),
                values: posts,
                hierarchy: Hierarchy::Prefix(Prefix {
                    lengths: vec![1, 2, 4],
                }),
            },
        ],
        3,
        NullPolicy::Wildcard,
    );
    insta::assert_yaml_snapshot!("realistic_release_report", out.report);
    insta::assert_yaml_snapshot!("realistic_release_classes", out.assessment.classes);
}
