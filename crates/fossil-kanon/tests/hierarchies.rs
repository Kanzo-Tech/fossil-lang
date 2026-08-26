//! The hierarchies in `hierarchies/` are data, and this is what makes that claim testable.
//!
//! Each file is read from disk, deserialised, and then *used* — a hierarchy that parses into a
//! value nobody generalises with is not a hierarchy, it is a fixture. The date tests double as the
//! correctness proof for the calendar arithmetic in `hierarchy::civil_from_days`, which is checked
//! through the public API rather than against itself: both Gregorian century rules, a leap day, and
//! a pre-epoch date.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, Date32Array, Int32Array, StringArray};
use fossil_kanon::hierarchy::{Date, DateLevel, Hierarchy, Numeric, NumericPresentation, Prefix};
use fossil_kanon::{Config, NullPolicy, QuasiIdentifier, anonymize};

fn load(name: &str) -> Hierarchy {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("hierarchies")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is a shipped hierarchy: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()))
}

/// Generalise one column with one hierarchy and return the published values in row order.
fn generalise(values: ArrayRef, hierarchy: Hierarchy, k: usize) -> Vec<String> {
    let out = anonymize(
        &[QuasiIdentifier {
            name: "qi".into(),
            values,
            hierarchy,
        }],
        &Config {
            k,
            nulls: NullPolicy::Wildcard,
        },
    )
    .expect("a well-formed run");
    let col = out.columns[0]
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    (0..col.len())
        .map(|i| {
            if col.is_null(i) {
                "<suppressed>".to_owned()
            } else {
                col.value(i).to_owned()
            }
        })
        .collect()
}

#[test]
fn the_shipped_hierarchies_are_the_three_kinds() {
    assert_eq!(
        load("uk-postcode.json"),
        Hierarchy::Prefix(Prefix {
            lengths: vec![1, 2, 4, 6]
        }),
        "area initial, area, outward code, sector"
    );
    assert_eq!(
        load("birth-date.json"),
        Hierarchy::Date(Date {
            levels: vec![DateLevel::Year, DateLevel::Month, DateLevel::Day]
        })
    );
    assert_eq!(
        load("age.json"),
        Hierarchy::Numeric(Numeric {
            buckets: vec![0.0, 18.0, 30.0, 45.0, 65.0, 80.0],
            presentation: NumericPresentation::EnclosingBucket
        })
    );
}

#[test]
fn a_hierarchy_round_trips_through_json() {
    for name in ["uk-postcode.json", "birth-date.json", "age.json"] {
        let h = load(name);
        let text = serde_json::to_string(&h).unwrap();
        assert_eq!(
            serde_json::from_str::<Hierarchy>(&text).unwrap(),
            h,
            "{name}"
        );
    }
}

#[test]
fn the_shipped_postcode_hierarchy_generalises_real_postcodes() {
    let posts: ArrayRef = Arc::new(StringArray::from(vec![
        "SW1A 1AA", "SW1A 2BB", "SW1P 3CC", "SW1P 4DD",
    ]));
    let out = generalise(posts, load("uk-postcode.json"), 2);
    // k = 2 lets it reach the outward code but no further: SW1A and SW1P are two of two.
    assert_eq!(out, vec!["SW1A*", "SW1A*", "SW1P*", "SW1P*"]);
}

#[test]
fn the_shipped_age_hierarchy_publishes_declared_buckets() {
    let ages: ArrayRef = Arc::new(Int32Array::from(vec![19, 22, 25, 47, 51, 55]));
    let out = generalise(ages, load("age.json"), 3);
    assert_eq!(
        out,
        vec![
            "[18, 30)", "[18, 30)", "[18, 30)", "[45, 65)", "[45, 65)", "[45, 65)"
        ]
    );
    // Not one of 19, 22, 25, 47, 51 or 55 appears anywhere in the release.
    for v in &out {
        for age in ["19", "22", "25", "47", "51", "55"] {
            assert!(!v.contains(age), "{v} leaks {age}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The calendar, through the shipped date hierarchy.
// ---------------------------------------------------------------------------------------------

/// Day numbers since the Unix epoch for dates that break naive calendar arithmetic.
const EPOCH: i32 = 0; // 1970-01-01
const DAY_BEFORE_EPOCH: i32 = -1; // 1969-12-31 — negative day numbers
const LEAP_2000: i32 = 11_016; // 2000-02-29 — divisible by 400, so a leap year
const MARCH_1900: i32 = -25_508; // 1900-03-01 — divisible by 100 but not 400, so NOT a leap year
const NEW_YEARS_EVE_2019: i32 = 18_261; // 2019-12-31

#[test]
fn the_shipped_date_hierarchy_renders_the_gregorian_calendar_correctly() {
    // Pairs, so k = 2 lets every class refine all the way to the day.
    let days: ArrayRef = Arc::new(Date32Array::from(vec![
        EPOCH,
        EPOCH,
        DAY_BEFORE_EPOCH,
        DAY_BEFORE_EPOCH,
        LEAP_2000,
        LEAP_2000,
        MARCH_1900,
        MARCH_1900,
        NEW_YEARS_EVE_2019,
        NEW_YEARS_EVE_2019,
    ]));
    assert_eq!(
        generalise(days, load("birth-date.json"), 2),
        vec![
            "1970-01-01",
            "1970-01-01",
            "1969-12-31",
            "1969-12-31",
            "2000-02-29",
            "2000-02-29",
            "1900-03-01",
            "1900-03-01",
            "2019-12-31",
            "2019-12-31",
        ]
    );
}

#[test]
fn a_date_hierarchy_stops_where_it_is_declared_to_stop() {
    // Year only. Even with k = 2 and two records per day, nothing finer than a year is published,
    // because the hierarchy declares no rung below it.
    let days: ArrayRef = Arc::new(Date32Array::from(vec![
        LEAP_2000,
        LEAP_2000,
        NEW_YEARS_EVE_2019,
        NEW_YEARS_EVE_2019,
    ]));
    assert_eq!(
        generalise(
            days,
            Hierarchy::Date(Date {
                levels: vec![DateLevel::Year]
            }),
            2
        ),
        vec!["2000", "2000", "2019", "2019"]
    );
}

#[test]
fn a_date_coarsens_to_the_month_when_k_does_not_allow_the_day() {
    // 2019-12-30 and 2019-12-31, one record each: the day level would give two classes of one, so
    // the refinement is refused and the month is published.
    let days: ArrayRef = Arc::new(Date32Array::from(vec![
        NEW_YEARS_EVE_2019 - 1,
        NEW_YEARS_EVE_2019,
    ]));
    assert_eq!(
        generalise(days, load("birth-date.json"), 2),
        vec!["2019-12", "2019-12"]
    );
}

// ---------------------------------------------------------------------------------------------
// Malformed declarations are refused, and the error names the column.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_malformed_hierarchy_is_refused_by_name() {
    let cases: Vec<(Hierarchy, &str)> = vec![
        (Hierarchy::Prefix(Prefix { lengths: vec![] }), "no levels"),
        (
            Hierarchy::Prefix(Prefix {
                lengths: vec![4, 2],
            }),
            "not ascending",
        ),
        (
            Hierarchy::Prefix(Prefix {
                lengths: vec![0, 2],
            }),
            "zero is the implicit top",
        ),
        (Hierarchy::Date(Date { levels: vec![] }), "no levels"),
        (
            Hierarchy::Date(Date {
                levels: vec![DateLevel::Day, DateLevel::Year],
            }),
            "not coarsest-first",
        ),
        (
            Hierarchy::Numeric(Numeric {
                buckets: vec![],
                presentation: NumericPresentation::EnclosingBucket,
            }),
            "buckets required",
        ),
        (
            Hierarchy::Numeric(Numeric {
                buckets: vec![30.0, 18.0],
                presentation: NumericPresentation::EnclosingBucket,
            }),
            "not ascending",
        ),
    ];
    for (hierarchy, why) in cases {
        let err = anonymize(
            &[QuasiIdentifier {
                name: "birthday".into(),
                values: Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
                hierarchy,
            }],
            &Config {
                k: 2,
                nulls: NullPolicy::Wildcard,
            },
        )
        .expect_err(why);
        assert!(
            err.to_string().contains("birthday"),
            "{why}: {err} does not name the column"
        );
    }
}
