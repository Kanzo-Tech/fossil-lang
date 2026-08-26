//! Does this release satisfy k? — answered over the released table alone.
//!
//! This half of the crate does not know what a hierarchy is, does not import one, and cannot
//! generalise anything. It takes the quasi-identifier columns of a table that has ALREADY been
//! published and reports the anonymity set of every equivalence class in it. That independence is
//! the point, and it is load-bearing twice over.
//!
//! # An auditor is not entitled to the sensitive column
//!
//! Checking that a release is k-anonymous needs the quasi-identifiers and nothing else: k-anonymity
//! is a property of the QI projection, and the sensitive attribute appears nowhere in its
//! definition. So a third party holding only `postcode`, `birth_year` and `sex` — parquet files, no
//! diagnosis column, no entitlement to one — can compute the compliance answer in full. Entangling
//! this with [`crate::anonymize`] would have made the check reachable only through a function that
//! wants the whole table, and the auditor would have had to ask for data they must not be given in
//! order to verify that the data they were given is safe.
//!
//! # A verifier that shares the generaliser's assumptions verifies nothing
//!
//! The second reason is ordinary engineering. [`crate::anonymize`] runs its own output through
//! [`assess_cells`] before returning it, and that check is worth something only because this module
//! re-derives the class sizes from the published strings rather than from the partitions that
//! produced them. A bug in the partitioner is visible here; a bug in a verifier that trusted the
//! partitioner's own bookkeeping would not be.
//!
//! # What it does NOT know, and why that is the safe direction
//!
//! It compares published cells as **strings**. It has no hierarchy, so it cannot see that `SW*`
//! subsumes `SW1A*`: to this module those are two unrelated values, and a row published as `SW1A*`
//! does not get credit for the rows published as `SW*` that an adversary who knows the postcode
//! system would consider indistinguishable from it.
//!
//! That under-counts anonymity sets, never over-counts them. A release this module passes is
//! k-anonymous under the string reading AND under any hierarchy reading, because every subsumption
//! it fails to see can only merge classes and make them larger. A release it fails may still be
//! compliant. Fail-safe is the only acceptable direction for the error here, and this is it.

use std::collections::HashMap;

use arrow_array::Array;
use arrow_array::cast::AsArray;
use arrow_schema::DataType;
use serde::Serialize;

use crate::Error;

/// One quasi-identifier column of an already-published table.
#[derive(Debug, Clone)]
pub struct ReleasedColumn {
    /// The column's name, for error messages and for the order of [`EquivalenceClass::values`].
    pub name: String,
    /// The published values. `Utf8`, `LargeUtf8` or `Utf8View` — a released quasi-identifier is a
    /// generalised value (`[18, 35)`, `SW1A*`, `2019-05`), and those are strings whatever the
    /// source column held.
    pub values: arrow_array::ArrayRef,
}

/// What counts as «this cell matches everything».
///
/// A missing quasi-identifier is a wildcard in disclosure-control semantics — it is compatible with
/// every value of the attribute, so it enlarges the anonymity set of every class it could belong to
/// rather than forming a class of its own. That reading is not the only one a published file can
/// carry, and guessing which one is in front of you is how a verifier reports a number that is not
/// about the table. So both halves of it are stated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WildcardPolicy {
    /// The published token that means «any value». `*` in every corpus this was written against.
    ///
    /// A cell equal to this string matches every other cell in its column.
    pub token: String,
    /// Whether an Arrow **null** in a published column reads as that same wildcard.
    ///
    /// `true` is the disclosure-control reading: the value is missing, so it is compatible with
    /// everything. `false` reads null as a value in its own right, which is the reading under which
    /// a file of null postcodes is one equivalence class rather than a class of every size at once.
    ///
    /// There is no default, because the two answers give different numbers on the same file and a
    /// verifier that picked one quietly would be reporting a compliance result about a table the
    /// auditor did not describe.
    pub null_is_wildcard: bool,
}

impl WildcardPolicy {
    /// `*` as the token, nulls read as wildcards — the reading in the k-anonymity literature and
    /// the one [`crate::anonymize`] publishes under.
    #[must_use]
    pub fn asterisk() -> Self {
        Self {
            token: "*".to_owned(),
            null_is_wildcard: true,
        }
    }
}

/// One published cell, as the verifier reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Cell {
    /// Matches every cell in its column, including another wildcard.
    Wildcard,
    /// Matches exactly the cells holding the same string.
    Value(String),
}

impl Cell {
    fn compatible(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Wildcard, _) | (_, Self::Wildcard) => true,
            (Self::Value(a), Self::Value(b)) => a == b,
        }
    }
}

/// An equivalence class of the published table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EquivalenceClass {
    /// The published tuple, in the column order of the input. A wildcard renders as the policy's
    /// token.
    pub values: Vec<String>,
    /// Rows whose published tuple is *exactly* this one.
    pub exact_rows: usize,
    /// Rows whose published tuple is *compatible* with this one — the anonymity set, and the
    /// quantity k is a bound on. Never smaller than [`Self::exact_rows`], and larger exactly when
    /// some other class carries a wildcard where this one carries a value.
    pub anonymity_set: usize,
}

/// The compliance answer for one released table.
///
/// `Serialize`, so an auditor's answer is a file rather than a screenful.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assessment {
    /// The k that was asked about.
    pub k: usize,
    /// Rows in the released table.
    pub rows: usize,
    /// Every distinct published tuple, ordered by [`EquivalenceClass::anonymity_set`] ascending —
    /// so the class that fails, if one does, is the first thing an auditor reads.
    pub classes: Vec<EquivalenceClass>,
    /// The smallest anonymity set over all classes. `None` for an empty table, which is the one
    /// case where the minimum is not a number and reporting `0` would be a claim about a class that
    /// does not exist.
    pub achieved_k: Option<usize>,
    /// Rows belonging to a class whose anonymity set is below k.
    pub rows_below_k: usize,
    /// Whether the release satisfies k. An empty table satisfies every k vacuously, and says so
    /// rather than dividing by a minimum it does not have.
    pub satisfies_k: bool,
}

/// Assess an already-published table.
///
/// Needs the quasi-identifier columns and nothing else — see the module docs on why that is a
/// property of the API rather than of this particular call.
///
/// # Errors
///
/// [`Error::NoQuasiIdentifiers`] for an empty column list, [`Error::LengthMismatch`] for columns of
/// differing lengths, [`Error::UnsupportedType`] for a column that is not a string type.
pub fn assess(
    columns: &[ReleasedColumn],
    k: usize,
    wildcards: &WildcardPolicy,
) -> Result<Assessment, Error> {
    let Some(first) = columns.first() else {
        return Err(Error::NoQuasiIdentifiers);
    };
    let rows = first.values.len();
    let mut cells = Vec::with_capacity(columns.len());
    for c in columns {
        if c.values.len() != rows {
            return Err(Error::LengthMismatch {
                column: c.name.clone(),
                len: c.values.len(),
                expected: rows,
            });
        }
        cells.push(read_column(c, wildcards)?);
    }
    Ok(assess_cells(&cells, rows, k, &wildcards.token))
}

fn read_column(column: &ReleasedColumn, wildcards: &WildcardPolicy) -> Result<Vec<Cell>, Error> {
    let read = |v: Option<&str>| match v {
        None if wildcards.null_is_wildcard => Cell::Wildcard,
        None => Cell::Value(String::new()),
        Some(s) if s == wildcards.token => Cell::Wildcard,
        Some(s) => Cell::Value(s.to_owned()),
    };
    let a = &column.values;
    Ok(match a.data_type() {
        DataType::Utf8 => {
            let a = a.as_string::<i32>();
            (0..a.len())
                .map(|i| read((!a.is_null(i)).then(|| a.value(i))))
                .collect()
        }
        DataType::LargeUtf8 => {
            let a = a.as_string::<i64>();
            (0..a.len())
                .map(|i| read((!a.is_null(i)).then(|| a.value(i))))
                .collect()
        }
        DataType::Utf8View => {
            let a = a.as_string_view();
            (0..a.len())
                .map(|i| read((!a.is_null(i)).then(|| a.value(i))))
                .collect()
        }
        other => {
            return Err(Error::UnsupportedType {
                column: column.name.clone(),
                data_type: other.to_string(),
                hierarchy: "released (string)",
            });
        }
    })
}

/// The whole of the computation, over columns of [`Cell`] — the form [`crate::anonymize`] already
/// holds its own output in, so it re-verifies through this function without a round trip through
/// Arrow.
///
/// # Cost
///
/// Grouping is linear. The compatibility pass is `O(|E| · |W|)` where `W` is the set of distinct
/// tuples carrying at least one wildcard and `E` those carrying none — so it is linear whenever the
/// table has no missing quasi-identifiers, and near-linear whenever it has few. It degrades to
/// quadratic in the distinct-tuple count on a table that is mostly wildcards, which is a table with
/// no disclosure risk to measure.
pub(crate) fn assess_cells(
    columns: &[Vec<Cell>],
    rows: usize,
    k: usize,
    token: &str,
) -> Assessment {
    let mut counts: HashMap<Vec<Cell>, usize> = HashMap::new();
    for row in 0..rows {
        let tuple: Vec<Cell> = columns.iter().map(|c| c[row].clone()).collect();
        *counts.entry(tuple).or_default() += 1;
    }

    let tuples: Vec<(Vec<Cell>, usize)> = counts.into_iter().collect();
    let (wild, exact): (Vec<usize>, Vec<usize>) = (0..tuples.len())
        .partition(|&i| tuples[i].0.iter().any(|c| *c == Cell::Wildcard));

    let mut classes: Vec<EquivalenceClass> = tuples
        .iter()
        .enumerate()
        .map(|(i, (tuple, count))| {
            // A tuple with no wildcard of its own is only ever enlarged by the wildcard-bearing
            // tuples; scanning the exact ones would be scanning for an equality already counted.
            let extra: usize = if wild.contains(&i) {
                exact
                    .iter()
                    .chain(wild.iter().filter(|&&j| j != i))
                    .filter(|&&j| compatible(tuple, &tuples[j].0))
                    .map(|&j| tuples[j].1)
                    .sum()
            } else {
                wild.iter()
                    .filter(|&&j| compatible(tuple, &tuples[j].0))
                    .map(|&j| tuples[j].1)
                    .sum()
            };
            EquivalenceClass {
                values: tuple
                    .iter()
                    .map(|c| match c {
                        Cell::Wildcard => token.to_owned(),
                        Cell::Value(v) => v.clone(),
                    })
                    .collect(),
                exact_rows: *count,
                anonymity_set: count + extra,
            }
        })
        .collect();

    // Ascending by anonymity set, then by the tuple itself. The second key is not cosmetic: a
    // HashMap iteration order is not stable between runs, and a report whose class list reorders
    // itself cannot be snapshot-tested or diffed between two releases of the same table.
    classes.sort_by(|a, b| {
        a.anonymity_set
            .cmp(&b.anonymity_set)
            .then_with(|| a.values.cmp(&b.values))
    });

    let achieved_k = classes.iter().map(|c| c.anonymity_set).min();
    let rows_below_k = classes
        .iter()
        .filter(|c| c.anonymity_set < k)
        .map(|c| c.exact_rows)
        .sum();

    Assessment {
        k,
        rows,
        achieved_k,
        rows_below_k,
        satisfies_k: achieved_k.is_none_or(|a| a >= k),
        classes,
    }
}

fn compatible(a: &[Cell], b: &[Cell]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.compatible(y))
}
