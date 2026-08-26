//! The verification half, used the way an auditor uses it.
//!
//! Note what this file does **not** import: `anonymize`, `Config`, `NullPolicy`, `QuasiIdentifier`,
//! and the whole `hierarchy` module. That is the test. An auditor holding the released
//! quasi-identifier Parquet — and no hierarchy, no configuration, and no entitlement to whatever
//! sensitive column the release was built around — can compute the compliance answer with what is
//! imported below and nothing else.
//!
//! If a future change makes `verify` need a hierarchy or a sensitive column, this file stops
//! compiling, which is the point of writing it this way rather than asserting it in prose.

use std::sync::Arc;

use arrow_array::{ArrayRef, StringArray};
use fossil_kanon::verify::{ReleasedColumn, WildcardPolicy, assess};

fn col(name: &str, values: Vec<Option<&str>>) -> ReleasedColumn {
    ReleasedColumn {
        name: name.to_owned(),
        values: Arc::new(StringArray::from(values)) as ArrayRef,
    }
}

fn released(rows: &[(&str, &str)]) -> Vec<ReleasedColumn> {
    vec![
        col("postcode", rows.iter().map(|r| Some(r.0)).collect()),
        col("age", rows.iter().map(|r| Some(r.1)).collect()),
    ]
}

#[test]
fn a_compliant_release_passes_and_reports_the_k_it_achieved() {
    let table = released(&[
        ("SW1*", "[18, 30)"),
        ("SW1*", "[18, 30)"),
        ("SW1*", "[18, 30)"),
        ("N1*", "[30, 45)"),
        ("N1*", "[30, 45)"),
        ("N1*", "[30, 45)"),
        ("N1*", "[30, 45)"),
    ]);
    let a = assess(&table, 3, &WildcardPolicy::asterisk()).unwrap();
    assert!(a.satisfies_k);
    assert_eq!(a.achieved_k, Some(3));
    assert_eq!(a.rows_below_k, 0);
    assert_eq!(a.classes.len(), 2);
    // Ascending by anonymity set, so the weakest class is the first thing read.
    assert_eq!(a.classes[0].anonymity_set, 3);
    assert_eq!(a.classes[1].anonymity_set, 4);
}

#[test]
fn a_short_class_fails_and_the_report_names_it_first() {
    let table = released(&[
        ("SW1*", "[18, 30)"),
        ("SW1*", "[18, 30)"),
        ("SW1*", "[18, 30)"),
        ("EC2*", "[65, 80)"),
    ]);
    let a = assess(&table, 3, &WildcardPolicy::asterisk()).unwrap();
    assert!(!a.satisfies_k);
    assert_eq!(a.achieved_k, Some(1));
    assert_eq!(a.rows_below_k, 1);
    assert_eq!(
        a.classes[0].values,
        vec!["EC2*".to_owned(), "[65, 80)".to_owned()]
    );
}

/// The wildcard reading is what makes the difference between «a class of one» and «a record
/// indistinguishable from every other». Both answers are about the same file.
#[test]
fn the_null_reading_changes_the_answer_which_is_why_it_has_no_default() {
    let table = vec![
        col(
            "postcode",
            vec![Some("SW1*"), Some("SW1*"), Some("SW1*"), None],
        ),
        col("age", vec![Some("[18, 30)"); 4]),
    ];

    let wildcard = assess(&table, 4, &WildcardPolicy::asterisk()).unwrap();
    assert!(wildcard.satisfies_k);
    assert_eq!(wildcard.achieved_k, Some(4));

    let own_value = assess(
        &table,
        4,
        &WildcardPolicy {
            token: "*".into(),
            null_is_wildcard: false,
        },
    )
    .unwrap();
    assert!(!own_value.satisfies_k);
    assert_eq!(own_value.achieved_k, Some(1));
}

/// A wildcard-bearing row enlarges every class it is compatible with, and its own anonymity set is
/// the union of them. It is not a class of its own, and it does not vanish.
#[test]
fn a_wildcard_enlarges_the_classes_it_matches_rather_than_forming_one() {
    let table = vec![
        col(
            "postcode",
            vec![
                Some("SW1*"),
                Some("SW1*"),
                Some("N1*"),
                Some("N1*"),
                Some("*"),
            ],
        ),
        col("age", vec![Some("[18, 30)"); 5]),
    ];
    let a = assess(&table, 3, &WildcardPolicy::asterisk()).unwrap();
    assert!(a.satisfies_k);
    for c in &a.classes {
        if c.values[0] == "*" {
            assert_eq!(c.exact_rows, 1);
            assert_eq!(c.anonymity_set, 5);
        } else {
            assert_eq!(c.exact_rows, 2);
            assert_eq!(c.anonymity_set, 3, "two of its own plus the wildcard");
        }
    }
}

/// The verifier has no hierarchy, so it does not know that `SW*` contains `SW1A*`. It therefore
/// counts them as unrelated values and reports a SMALLER anonymity set than an adversary who knows
/// the postcode system would face. Under-counting is the safe direction and this pins it.
#[test]
fn subsumption_between_generalised_values_is_not_inferred_and_that_under_counts() {
    let table = vec![
        col(
            "postcode",
            vec![Some("SW*"), Some("SW*"), Some("SW1A*"), Some("SW1A*")],
        ),
        col("age", vec![Some("*"); 4]),
    ];
    let a = assess(&table, 4, &WildcardPolicy::asterisk()).unwrap();
    // A hierarchy-aware reading would say every row is indistinguishable from all four.
    assert!(!a.satisfies_k);
    assert_eq!(a.achieved_k, Some(2));
    assert_eq!(a.classes.len(), 2);
}

#[test]
fn a_custom_wildcard_token_is_honoured() {
    let table = vec![col(
        "postcode",
        vec![Some("ANY"), Some("SW1*"), Some("SW1*")],
    )];
    let a = assess(
        &table,
        3,
        &WildcardPolicy {
            token: "ANY".into(),
            null_is_wildcard: true,
        },
    )
    .unwrap();
    assert!(a.satisfies_k);
    assert_eq!(a.achieved_k, Some(3));
}

#[test]
fn an_empty_release_satisfies_every_k_and_says_so_without_a_minimum() {
    let a = assess(&[col("postcode", vec![])], 5, &WildcardPolicy::asterisk()).unwrap();
    assert!(a.satisfies_k);
    assert_eq!(a.achieved_k, None);
    assert_eq!(a.rows, 0);
}

#[test]
fn ragged_columns_are_refused() {
    let table = vec![
        col("postcode", vec![Some("SW1*"), Some("SW1*")]),
        col("age", vec![Some("[18, 30)")]),
    ];
    assert!(assess(&table, 2, &WildcardPolicy::asterisk()).is_err());
}

/// The class listing is ordered deterministically, so two audits of the same file diff cleanly and
/// a snapshot of one is stable. `HashMap` iteration order is not, and the sort is what covers it.
#[test]
fn the_class_listing_is_stable_across_runs() {
    let table = released(&[
        ("SW1*", "[18, 30)"),
        ("SW1*", "[18, 30)"),
        ("N1*", "[30, 45)"),
        ("N1*", "[30, 45)"),
        ("EC2*", "[65, 80)"),
        ("EC2*", "[65, 80)"),
    ]);
    let first = assess(&table, 2, &WildcardPolicy::asterisk()).unwrap();
    for _ in 0..25 {
        assert_eq!(
            assess(&table, 2, &WildcardPolicy::asterisk()).unwrap(),
            first
        );
    }
    insta::assert_yaml_snapshot!("stable_class_listing", first);
}
