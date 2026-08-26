//! The contract, asserted rather than assumed.
//!
//! > For any input and any k, every emitted equivalence class has at least k members, **or** its
//! > records are counted as suppressed.
//!
//! That sentence is the whole of what this crate promises, and it is universally quantified. A
//! privacy guarantee asserted by example is a guarantee asserted over the inputs its author thought
//! of, and the inputs that break this algorithm are precisely the ones nobody writes down: the
//! median value with k duplicates behind it, the column that is one value repeated, the k larger
//! than the table, the table that is entirely nulls, the table with no rows at all.
//!
//! The second property here is the one that makes the first worth having. The invariant is checked
//! by [`fossil_kanon::verify::assess`] — the **public** verifier, over the **Arrow output**, with no
//! access to the partitions that produced it. If the partitioner and the verifier ever agreed by
//! sharing a mistake, this is where it would show.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, Date32Array, Int32Array, StringArray};
use fossil_kanon::hierarchy::{Date, DateLevel, Hierarchy, Numeric, Prefix};
use fossil_kanon::verify::{ReleasedColumn, WildcardPolicy, assess};
use fossil_kanon::{Anonymized, Config, NullPolicy, QuasiIdentifier, anonymize};
use proptest::prelude::*;

/// Ages, postcodes and birth dates — one column per hierarchy kind, all of one length.
type Table = (Vec<Option<i32>>, Vec<Option<String>>, Vec<Option<i32>>);

/// Three columns of one length, one of each hierarchy kind — the three shapes the partitioner has
/// separate code paths for, exercised together so a cut on one interacts with a cut on another.
fn table() -> impl Strategy<Value = Table> {
    (0usize..40).prop_flat_map(|n| {
        (
            prop::collection::vec(prop::option::of(0i32..90), n),
            // A deliberately small alphabet: distinct postcodes in a table of forty rows would make
            // every class a singleton and every run a trivial one. The interesting inputs are the
            // ones with ties.
            prop::collection::vec(prop::option::of("[A-C][1-3]"), n),
            prop::collection::vec(prop::option::of(-1000i32..2000), n),
        )
    })
}

fn qis(
    ages: &[Option<i32>],
    posts: &[Option<String>],
    dates: &[Option<i32>],
) -> Vec<QuasiIdentifier> {
    let age: ArrayRef = Arc::new(Int32Array::from(ages.to_vec()));
    let post: ArrayRef = Arc::new(StringArray::from(posts.to_vec()));
    let date: ArrayRef = Arc::new(Date32Array::from(dates.to_vec()));
    vec![
        QuasiIdentifier {
            name: "age".into(),
            values: age,
            hierarchy: Hierarchy::Numeric(Numeric::bucketed(vec![
                0.0, 18.0, 30.0, 45.0, 65.0, 80.0,
            ])),
        },
        QuasiIdentifier {
            name: "postcode".into(),
            values: post,
            hierarchy: Hierarchy::Prefix(Prefix {
                lengths: vec![1, 2],
            }),
        },
        QuasiIdentifier {
            name: "born".into(),
            values: date,
            hierarchy: Hierarchy::Date(Date {
                levels: vec![DateLevel::Year, DateLevel::Month, DateLevel::Day],
            }),
        },
    ]
}

/// The released rows of the output, as an independent auditor would receive them: the published
/// Arrow columns, projected to the rows that were actually released, and nothing else.
fn as_released(out: &Anonymized, names: &[&str]) -> Vec<ReleasedColumn> {
    out.columns
        .iter()
        .zip(names)
        .map(|(col, name)| {
            let col = col
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("anonymize publishes StringArray");
            let kept: Vec<Option<String>> = (0..col.len())
                .filter(|&i| out.released[i])
                .map(|i| (!col.is_null(i)).then(|| col.value(i).to_owned()))
                .collect();
            ReleasedColumn {
                name: (*name).to_owned(),
                values: Arc::new(StringArray::from(kept)) as ArrayRef,
            }
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// THE CONTRACT.
    #[test]
    fn every_released_row_is_in_a_class_of_at_least_k_and_the_rest_are_counted_as_suppressed(
        (ages, posts, dates) in table(),
        k in 2usize..9,
        suppress in any::<bool>(),
    ) {
        let nulls = if suppress { NullPolicy::Suppress } else { NullPolicy::Wildcard };
        let out = anonymize(&qis(&ages, &posts, &dates), &Config { k, nulls }).unwrap();
        let r = &out.report;

        // Every emitted class reaches k…
        for c in &out.assessment.classes {
            prop_assert!(
                c.anonymity_set >= k,
                "class {:?} has an anonymity set of {} against k = {k}",
                c.values, c.anonymity_set
            );
        }
        prop_assert!(out.assessment.satisfies_k);
        prop_assert_eq!(out.assessment.rows_below_k, 0);

        // …or its records are counted as suppressed, and the accounting is exact.
        prop_assert_eq!(r.input_rows, ages.len());
        prop_assert_eq!(r.released_rows + r.suppressed_rows, r.input_rows);
        prop_assert_eq!(
            r.suppressed_rows,
            r.suppressed_by_null_policy + r.suppressed_below_k + r.suppressed_by_safety_net
        );
        prop_assert_eq!(r.released_rows, out.released.iter().filter(|b| **b).count());

        // Strict Mondrian cannot emit a short class, so the net that catches one must never fire.
        // A failure here is a bug in the partitioner that this crate refused to publish.
        prop_assert_eq!(
            r.suppressed_by_safety_net, 0,
            "the output verifier withheld rows the partitioner had released"
        );

        if r.released_rows > 0 {
            prop_assert!(r.achieved_k.unwrap() >= k);
        } else {
            prop_assert_eq!(r.achieved_k, None);
        }
    }

    /// The public verifier, over the published Arrow columns, reaches the same answer as the
    /// assessment `anonymize` returned. The two halves of the crate agree without sharing state.
    #[test]
    fn the_standalone_verifier_confirms_the_derived_release(
        (ages, posts, dates) in table(),
        k in 2usize..9,
    ) {
        let out = anonymize(
            &qis(&ages, &posts, &dates),
            &Config { k, nulls: NullPolicy::Wildcard },
        ).unwrap();
        if out.report.released_rows == 0 {
            return Ok(());
        }
        let audited = assess(
            &as_released(&out, &["age", "postcode", "born"]),
            k,
            &WildcardPolicy::asterisk(),
        ).unwrap();

        prop_assert!(audited.satisfies_k);
        prop_assert_eq!(audited.rows, out.report.released_rows);
        prop_assert_eq!(audited.achieved_k, out.report.achieved_k);
        prop_assert_eq!(audited.classes.len(), out.report.equivalence_classes);
    }

    /// A suppressed row is absent from every published column, and a released one is present in all
    /// of them. A row half-released would be a row whose class is not the class it was counted in.
    #[test]
    fn a_row_is_released_in_every_column_or_in_none(
        (ages, posts, dates) in table(),
        k in 2usize..9,
        suppress in any::<bool>(),
    ) {
        let nulls = if suppress { NullPolicy::Suppress } else { NullPolicy::Wildcard };
        let out = anonymize(&qis(&ages, &posts, &dates), &Config { k, nulls }).unwrap();
        for col in &out.columns {
            let col = col.as_any().downcast_ref::<StringArray>().unwrap();
            prop_assert_eq!(col.len(), out.released.len());
            for (i, &released) in out.released.iter().enumerate() {
                prop_assert_eq!(!col.is_null(i), released);
            }
        }
    }
}
