//! Generalisation hierarchies, as declared data.
//!
//! Three kinds, and the count is deliberate rather than a stopping point: a date coarsened
//! day → month → year, a string truncated to a prefix, and a number placed in a bucket. Between
//! them they cover the quasi-identifiers that actually appear in the re-identification literature
//! — birth date, postcode, age — which is the set Sweeney's 87 % result was computed over.
//!
//! They are **data**, not code. Amnesia ships its hierarchies as files, and that is the shape worth
//! copying: a postcode hierarchy is a fact about a country's postcode system, revised by
//! statisticians and not by programmers, and a recompile is the wrong unit of change for it.
//! `hierarchies/` in this crate holds three, and [`Hierarchy`] is `Deserialize` so that directory is
//! the interface rather than an illustration of one.
//!
//! # Every hierarchy has an implicit top, and it is `*`
//!
//! Level 0 is «the whole domain», published as `*`, and it is not written down in any of these
//! structs — the declared levels are the refinements BELOW it. The root partition sits at level 0 on
//! every dimension, which is the only starting point at which the table is trivially k-anonymous for
//! every k up to its own row count. A hierarchy whose coarsest declared level were something
//! narrower would start the search at a point that is not safe, and Mondrian only ever refines.

use serde::{Deserialize, Serialize};

/// How one quasi-identifier column is generalised.
///
/// Deserialises from a tagged object — `{"kind": "prefix", "lengths": [1, 2, 4]}` — so a hierarchy
/// can be a file the caller ships beside their data rather than a literal in their program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hierarchy {
    /// An ordered numeric domain, cut at medians. See [`Numeric`].
    Numeric(Numeric),
    /// A string truncated to a declared prefix length. See [`Prefix`].
    Prefix(Prefix),
    /// A date coarsened to month or year. See [`Date`].
    Date(Date),
}

impl Hierarchy {
    pub(crate) fn validate(&self, column: &str) -> Result<(), crate::Error> {
        match self {
            Self::Numeric(n) => n.validate(column),
            Self::Prefix(p) => p.validate(column),
            Self::Date(d) => d.validate(column),
        }
    }
}

/// An ordered numeric domain.
///
/// This is the one kind that is not levelled, and the difference is the whole of the local-recoding
/// problem. Mondrian cuts a numeric dimension at the median of the values *in the partition*, so two
/// sibling partitions end up with two spans that were never declared anywhere and that stand in no
/// containment relation to each other. There is no «level» to report for such a column, and
/// [`crate::ColumnLevels`] refuses to invent one.
///
/// [`buckets`](Self::buckets) is the answer to the other half of the problem — see
/// [`NumericPresentation`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Numeric {
    /// Bucket edges, strictly ascending, in the units of the column. `[0.0, 18.0, 35.0, 65.0]`
    /// declares four spans: below 18, 18–35, 35–65, and 65 and up.
    ///
    /// Empty is legal and means the caller has declined to declare a domain, which forces
    /// [`NumericPresentation::ObservedRange`].
    #[serde(default)]
    pub buckets: Vec<f64>,
    /// What a generalised value looks like when it is published.
    pub presentation: NumericPresentation,
}

impl Numeric {
    /// Bucketed publication over the given edges — the presentation that leaks no record.
    #[must_use]
    pub const fn bucketed(buckets: Vec<f64>) -> Self {
        Self {
            buckets,
            presentation: NumericPresentation::EnclosingBucket,
        }
    }

    fn validate(&self, column: &str) -> Result<(), crate::Error> {
        if !self.buckets.windows(2).all(|w| w[0] < w[1])
            || self.buckets.iter().any(|b| !b.is_finite())
        {
            return Err(crate::Error::BucketsNotAscending {
                column: column.to_owned(),
            });
        }
        if self.presentation == NumericPresentation::EnclosingBucket && self.buckets.is_empty() {
            return Err(crate::Error::BucketsRequired {
                column: column.to_owned(),
            });
        }
        Ok(())
    }
}

/// How a generalised numeric span reaches the published table.
///
/// # The endpoints of `[min, max]` are two real people
///
/// Mondrian's own output for a numeric dimension is the interval between the smallest and largest
/// value in the partition, and both of those are values that a specific record actually holds. A
/// published `[29, 61]` says, exactly, «someone in this class is 29 and someone in this class is
/// 61» — two attributes released in full, in a column whose entire purpose was to be generalised.
/// It is worse where the class is small, which is where k-anonymity puts its attention.
///
/// So the choice is a named parameter and not a formatting detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericPresentation {
    /// Publish the narrowest span of DECLARED bucket edges that encloses the partition, e.g.
    /// `[18, 35)` for a partition whose observed values run 29 to 33.
    ///
    /// The published value is then a function of the hierarchy and not of the data, which is what
    /// makes it safe: it names no record, and two partitions that land in the same bucket publish
    /// the same string and merge into one — larger — equivalence class. Bucketing can only raise
    /// the achieved k, never lower it, because it is a coarsening applied after the cut.
    ///
    /// Requires non-empty [`Numeric::buckets`].
    EnclosingBucket,
    /// Publish `[min, max]` over the partition — Mondrian's own output, and two real record values.
    ///
    /// Offered because it is what the literature reports utility numbers against, and refusing it
    /// would make this crate incomparable with every published Mondrian benchmark. Choosing it
    /// raises [`crate::Warning::ObservedRangeEndpoints`] on every run, and the warning does not go
    /// away with familiarity.
    ObservedRange,
}

/// A string generalised by truncation to a declared prefix length.
///
/// Postcodes are the case this exists for: `SW1A 1AA` → `SW1A*` → `SW1*` → `SW*` → `*`, which is a
/// real containment hierarchy because the UK postcode system is itself hierarchical. It is NOT a
/// general-purpose categorical hierarchy, and the difference matters — see the crate docs on what a
/// caller must not assume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefix {
    /// Prefix lengths in **characters**, strictly ascending, all non-zero. `[1, 2, 4]` declares
    /// three levels below the implicit `*`.
    ///
    /// Characters, not bytes: a `Vec<char>` truncation cannot split a code point, and a postcode
    /// hierarchy that panics on an accented address is not one.
    pub lengths: Vec<usize>,
}

impl Prefix {
    fn validate(&self, column: &str) -> Result<(), crate::Error> {
        if self.lengths.is_empty() {
            return Err(crate::Error::EmptyHierarchy {
                column: column.to_owned(),
            });
        }
        if self.lengths[0] == 0 || !self.lengths.windows(2).all(|w| w[0] < w[1]) {
            return Err(crate::Error::PrefixLengthsNotAscending {
                column: column.to_owned(),
            });
        }
        Ok(())
    }

    /// The value of `s` at level `level` (1-based; level 0 is the implicit top and is never asked
    /// for here).
    ///
    /// Always suffixed with `*`, including at the longest declared level. That is not a cosmetic
    /// choice: values in the column vary in length, so a prefix of length 4 is the whole of `SW1A`
    /// and a truncation of `SW1A 1AA`, and a rendering that dropped the star at the finest level
    /// would publish one class value that means two different things. `SW1A*` means «anything
    /// extending SW1A» in both cases, which is true in both cases.
    pub(crate) fn value_at(&self, level: usize, s: &str) -> String {
        let n = self.lengths[level - 1];
        let mut out: String = s.chars().take(n).collect();
        out.push('*');
        out
    }

    pub(crate) const fn depth(&self) -> usize {
        self.lengths.len()
    }
}

/// A date coarsened towards the year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Date {
    /// Levels from coarsest to finest, below the implicit `*`. `[Year, Month, Day]` is the full
    /// hierarchy; `[Year]` publishes nothing finer than a year no matter how large k allows.
    pub levels: Vec<DateLevel>,
}

impl Date {
    fn validate(&self, column: &str) -> Result<(), crate::Error> {
        if self.levels.is_empty() {
            return Err(crate::Error::EmptyHierarchy {
                column: column.to_owned(),
            });
        }
        if !self.levels.windows(2).all(|w| w[0] < w[1]) {
            return Err(crate::Error::DateLevelsNotDescending {
                column: column.to_owned(),
            });
        }
        Ok(())
    }

    /// The value of `days` (days since the Unix epoch) at level `level` (1-based).
    pub(crate) fn value_at(&self, level: usize, days: i32) -> String {
        let (y, m, d) = civil_from_days(days);
        match self.levels[level - 1] {
            DateLevel::Year => format!("{y:04}"),
            DateLevel::Month => format!("{y:04}-{m:02}"),
            DateLevel::Day => format!("{y:04}-{m:02}-{d:02}"),
        }
    }

    pub(crate) const fn depth(&self) -> usize {
        self.levels.len()
    }
}

/// One rung of a [`Date`] hierarchy. Ordered coarsest-first, which is what makes the `Ord` derive
/// usable as the ascending check in [`Date::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateLevel {
    /// `2019`.
    Year,
    /// `2019-05`.
    Month,
    /// `2019-05-17`.
    Day,
}

/// Days since the Unix epoch → `(year, month, day)` in the proleptic Gregorian calendar.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every value in ±32767 years and is
/// twenty lines of integer arithmetic. `chrono` and `time` both do this correctly and both are a
/// dependency; this crate holds four, one of which is `thiserror`, and a date hierarchy is not
/// worth a fifth. The correctness of this function is not taken on trust — `tests/hierarchies.rs`
/// checks it against the epoch, both Gregorian century rules, and every leap-day boundary in a
/// four-century cycle.
#[expect(
    clippy::similar_names,
    reason = "`doe`/`doy`/`yoe` are Hinnant's own names — day-of-era, day-of-year, year-of-era — and               renaming them for legibility is what would make this unreviewable against the source"
)]
fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    #[expect(
        clippy::cast_sign_loss,
        reason = "d is in [1,31] and m in [1,12] by construction — the ranges are the comments above"
    )]
    (y + i32::from(m <= 2), m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_the_first_of_january_nineteen_seventy() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn a_prefix_level_keeps_its_star_at_the_finest_rung() {
        let p = Prefix {
            lengths: vec![2, 4],
        };
        assert_eq!(p.value_at(1, "SW1A 1AA"), "SW*");
        assert_eq!(p.value_at(2, "SW1A 1AA"), "SW1A*");
        // A value SHORTER than the declared level is not padded and is not an error: `EC*` at
        // level 2 means «anything extending EC», which is exactly what is known about it.
        assert_eq!(p.value_at(2, "EC"), "EC*");
    }

    #[test]
    fn a_prefix_level_cuts_on_characters_and_not_on_bytes() {
        let p = Prefix { lengths: vec![3] };
        assert_eq!(p.value_at(1, "ÑOÑO"), "ÑOÑ*");
    }

    #[test]
    fn enclosing_bucket_without_buckets_is_a_configuration_error() {
        let n = Numeric {
            buckets: vec![],
            presentation: NumericPresentation::EnclosingBucket,
        };
        assert!(matches!(
            n.validate("age"),
            Err(crate::Error::BucketsRequired { .. })
        ));
    }
}
