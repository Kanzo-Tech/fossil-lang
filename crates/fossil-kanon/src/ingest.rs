//! Arrow arrays → the two shapes the partitioner understands.
//!
//! There are exactly two: an ordered domain of `f64`, and a ladder of pre-rendered level strings.
//! Everything a hierarchy declares is resolved here, once, before the search starts — a partition
//! refinement is then a group-by over a `Vec<Option<String>>` and never a call back into a
//! hierarchy. That is not only speed: it means a hierarchy is consulted a fixed number of times per
//! column and cannot be consulted differently in two places.
//!
//! # Why the level strings are materialised
//!
//! Rendering level `i` of row `r` on demand is O(1) and would save the memory. It also puts the
//! hierarchy inside the inner loop of a recursive search, where the cost of getting it wrong is a
//! table that is not k-anonymous rather than a table that is slow. `rows × depth` `String`s is at
//! most five per row for the postcode hierarchy, and the crate is aimed at a browser tab holding a
//! table it can already fit.

use std::collections::HashSet;

use arrow_array::cast::AsArray;
use arrow_array::{Array, ArrayRef, types};
use arrow_schema::DataType;

use crate::hierarchy::{Date, Hierarchy, NumericPresentation, Prefix};
use crate::{Error, QuasiIdentifier};

/// One quasi-identifier column, resolved.
#[derive(Debug)]
pub(crate) struct Dim {
    pub(crate) name: String,
    pub(crate) values: Values,
}

#[derive(Debug)]
pub(crate) enum Values {
    /// An ordered domain. `global` is the (min, max) over the non-null values, used to normalise
    /// this dimension's width against the others when Mondrian chooses where to cut.
    Numeric {
        values: Vec<Option<f64>>,
        global: Option<(f64, f64)>,
        buckets: Vec<f64>,
        presentation: NumericPresentation,
    },
    /// A ladder. `levels[i]` holds every row's value at hierarchy level `i + 1`; level 0 is the
    /// implicit `*` and is not stored.
    Levelled {
        levels: Vec<Vec<Option<String>>>,
        /// Distinct values at the FINEST declared level, over the whole input. The denominator for
        /// this dimension's normalised width — Mondrian's `len(leaves)` for a categorical
        /// attribute, and the only reading of «how wide is this column» that is comparable with a
        /// numeric span.
        leaf_distinct: usize,
    },
}

impl Values {
    /// A row is null on this dimension when its source value was null. For a ladder that is true at
    /// every level at once, so the finest level answers for all of them.
    pub(crate) fn is_null(&self, row: usize) -> bool {
        match self {
            Self::Numeric { values, .. } => values[row].is_none(),
            Self::Levelled { levels, .. } => levels[levels.len() - 1][row].is_none(),
        }
    }

    pub(crate) const fn depth(&self) -> usize {
        match self {
            Self::Numeric { .. } => 0,
            Self::Levelled { levels, .. } => levels.len(),
        }
    }
}

/// Resolve every declared quasi-identifier against its array.
pub(crate) fn resolve(qis: &[QuasiIdentifier], rows: usize) -> Result<Vec<Dim>, Error> {
    qis.iter()
        .map(|qi| {
            qi.hierarchy.validate(&qi.name)?;
            if qi.values.len() != rows {
                return Err(Error::LengthMismatch {
                    column: qi.name.clone(),
                    len: qi.values.len(),
                    expected: rows,
                });
            }
            let values = match &qi.hierarchy {
                Hierarchy::Numeric(n) => numeric(&qi.name, &qi.values, n)?,
                Hierarchy::Prefix(p) => prefix(&qi.name, &qi.values, p)?,
                Hierarchy::Date(d) => date(&qi.name, &qi.values, d)?,
            };
            Ok(Dim {
                name: qi.name.clone(),
                values,
            })
        })
        .collect()
}

/// The largest integer an `f64` represents exactly. An identifier column past this is refused
/// rather than rounded: two records rounded to the same `f64` would be published as one
/// equivalence class member each while being, in the source, two distinguishable values — the
/// achieved k would be reported over a domain that is not the caller's.
const EXACT_INTEGER_LIMIT: i128 = 1 << 53;

fn numeric(
    column: &str,
    array: &ArrayRef,
    decl: &crate::hierarchy::Numeric,
) -> Result<Values, Error> {
    macro_rules! from_int {
        ($ty:ty) => {{
            let a = array.as_primitive::<$ty>();
            let mut out = Vec::with_capacity(a.len());
            for i in 0..a.len() {
                if a.is_null(i) {
                    out.push(None);
                } else {
                    let v = i128::from(a.value(i));
                    if v.abs() > EXACT_INTEGER_LIMIT {
                        return Err(Error::IntegerNotExact {
                            column: column.to_owned(),
                            value: v,
                        });
                    }
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "guarded one line above: |v| <= 2^53, where f64 is exact"
                    )]
                    out.push(Some(v as f64));
                }
            }
            out
        }};
    }

    let values: Vec<Option<f64>> = match array.data_type() {
        DataType::Int8 => from_int!(types::Int8Type),
        DataType::Int16 => from_int!(types::Int16Type),
        DataType::Int32 => from_int!(types::Int32Type),
        DataType::Int64 => from_int!(types::Int64Type),
        DataType::UInt8 => from_int!(types::UInt8Type),
        DataType::UInt16 => from_int!(types::UInt16Type),
        DataType::UInt32 => from_int!(types::UInt32Type),
        DataType::UInt64 => from_int!(types::UInt64Type),
        DataType::Float32 => {
            let a = array.as_primitive::<types::Float32Type>();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| f64::from(a.value(i))))
                .collect()
        }
        DataType::Float64 => {
            let a = array.as_primitive::<types::Float64Type>();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| a.value(i)))
                .collect()
        }
        other => {
            return Err(Error::UnsupportedType {
                column: column.to_owned(),
                data_type: other.to_string(),
                hierarchy: "numeric",
            });
        }
    };

    // NaN is not a member of an ordered domain: it compares false against everything, so it would
    // sort arbitrarily, land in an arbitrary partition, and widen a published span to an interval
    // whose endpoints are not the values it contains. It is not data missing in the Arrow sense
    // either, so it is refused rather than silently read as null — a caller who means «unknown»
    // has a null bit to say so with, and NullPolicy to say what it means.
    if let Some(i) = values.iter().position(|v| v.is_some_and(f64::is_nan)) {
        return Err(Error::NotANumber {
            column: column.to_owned(),
            row: i,
        });
    }

    let global = values.iter().flatten().fold(None, |acc, &v| match acc {
        None => Some((v, v)),
        Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
    });

    Ok(Values::Numeric {
        values,
        global,
        buckets: decl.buckets.clone(),
        presentation: decl.presentation,
    })
}

fn prefix(column: &str, array: &ArrayRef, decl: &Prefix) -> Result<Values, Error> {
    let raw: Vec<Option<&str>> = match array.data_type() {
        DataType::Utf8 => {
            let a = array.as_string::<i32>();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| a.value(i)))
                .collect()
        }
        DataType::LargeUtf8 => {
            let a = array.as_string::<i64>();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| a.value(i)))
                .collect()
        }
        DataType::Utf8View => {
            let a = array.as_string_view();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| a.value(i)))
                .collect()
        }
        other => {
            return Err(Error::UnsupportedType {
                column: column.to_owned(),
                data_type: other.to_string(),
                hierarchy: "prefix",
            });
        }
    };

    Ok(ladder(
        (1..=decl.depth())
            .map(|level| {
                raw.iter()
                    .map(|v| v.map(|s| decl.value_at(level, s)))
                    .collect()
            })
            .collect(),
    ))
}

fn date(column: &str, array: &ArrayRef, decl: &Date) -> Result<Values, Error> {
    /// Arrow's `Date64` counts milliseconds, and only multiples of a day are meaningful in it.
    /// `div_euclid` rather than `/` so a pre-epoch date floors towards the earlier day instead of
    /// truncating towards zero, which would put 1969-12-31 23:00 on 1970-01-01.
    const MS_PER_DAY: i64 = 86_400_000;

    let raw: Vec<Option<i32>> = match array.data_type() {
        DataType::Date32 => {
            let a = array.as_primitive::<types::Date32Type>();
            (0..a.len())
                .map(|i| (!a.is_null(i)).then(|| a.value(i)))
                .collect()
        }
        DataType::Date64 => {
            let a = array.as_primitive::<types::Date64Type>();
            let mut out = Vec::with_capacity(a.len());
            for i in 0..a.len() {
                if a.is_null(i) {
                    out.push(None);
                } else {
                    let days = a.value(i).div_euclid(MS_PER_DAY);
                    out.push(Some(i32::try_from(days).map_err(|_| {
                        Error::DateOutOfRange {
                            column: column.to_owned(),
                            row: i,
                        }
                    })?));
                }
            }
            out
        }
        other => {
            return Err(Error::UnsupportedType {
                column: column.to_owned(),
                data_type: other.to_string(),
                hierarchy: "date",
            });
        }
    };

    Ok(ladder(
        (1..=decl.depth())
            .map(|level| {
                raw.iter()
                    .map(|v| v.map(|d| decl.value_at(level, d)))
                    .collect()
            })
            .collect(),
    ))
}

fn ladder(levels: Vec<Vec<Option<String>>>) -> Values {
    let leaf_distinct = levels
        .last()
        .map(|leaf| leaf.iter().flatten().collect::<HashSet<_>>().len())
        .unwrap_or_default();
    Values::Levelled {
        levels,
        leaf_distinct,
    }
}
