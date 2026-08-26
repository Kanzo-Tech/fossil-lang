//! What the run did, in a shape that cannot overstate what it achieved.
//!
//! Two fields carry most of the weight. [`Report::achieved_k`] is measured by [`crate::verify`]
//! over the published strings rather than counted by the partitioner, so it is an independent
//! reading of the same table. And [`ColumnLevels`] refuses to report a hierarchy level for a
//! numeric column, because there isn't one — see its own docs.

use crate::hierarchy::NumericPresentation;

/// The outcome of one [`crate::anonymize`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// The k that was asked for.
    pub k: usize,
    /// Rows handed in.
    pub input_rows: usize,
    /// Rows in the published table.
    pub released_rows: usize,
    /// Rows withheld. The sum of the two fields below, and the number that matters to a caller
    /// deciding whether the release is still worth publishing.
    pub suppressed_rows: usize,
    /// Rows withheld because they carried a missing quasi-identifier and
    /// [`crate::NullPolicy::Suppress`] was in force.
    pub suppressed_by_null_policy: usize,
    /// Rows withheld because the published table did not satisfy k after partitioning.
    ///
    /// **This should always be zero, and it is reported because «should» is not «is».** Strict
    /// Mondrian only ever performs a cut whose every side already has k members, so a final
    /// partition below k is a contradiction of the algorithm rather than an expected outcome. The
    /// crate nonetheless verifies its own output and withholds anything that fails, so the
    /// guarantee holds unconditionally rather than conditionally on the partitioner being correct.
    /// A non-zero value here is a bug in this crate that did not become a disclosure, and
    /// [`Warning::SafetyNetFired`] accompanies it.
    pub suppressed_by_safety_net: usize,
    /// Distinct published tuples.
    pub equivalence_classes: usize,
    /// The smallest anonymity set in the published table — the k actually achieved, which is at
    /// least the k requested whenever anything was released at all. `None` when nothing was.
    pub achieved_k: Option<usize>,
    /// The smallest count of rows sharing *exactly* one published tuple. Below
    /// [`Self::achieved_k`] exactly when wildcards are in play, and reported separately because a
    /// caller who reads k under strict tuple equality is reading this number.
    pub smallest_exact_class: Option<usize>,
    /// How far each quasi-identifier was generalised.
    pub columns: Vec<ColumnReport>,
    /// Everything the run wants the caller to know and could not express by failing.
    pub warnings: Vec<Warning>,
}

/// How far one quasi-identifier was generalised.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnReport {
    /// The column, named as the caller named it.
    pub name: String,
    /// See [`ColumnLevels`].
    pub levels: ColumnLevels,
}

/// The generalisation reached on one column — and the two kinds are not interchangeable.
///
/// # Local recoding does not produce a hierarchy level
///
/// This enum exists so that the report cannot pretend otherwise. Mondrian cuts a numeric dimension
/// at the median of the values *inside a partition*, so each partition ends up with a span that was
/// computed from its own contents. Two sibling partitions can publish `[29, 44]` and `[45, 61]`; a
/// third, elsewhere in the tree, `[31, 58]`. Those three spans are not three values of one
/// attribute at one level of one hierarchy. They do not nest, they overlap, and there is no
/// generalisation lattice in which they are siblings.
///
/// The consequence for a caller is concrete: **you cannot join two Mondrian outputs on a
/// generalised numeric column, you cannot compare «the level reached» between two runs, and you
/// cannot roll one output up into another.** A levelled column ([`Self::Levels`]) supports all
/// three, because its published values come from a hierarchy that was declared before the data was
/// seen. A numeric column reports [`Self::Spans`] and offers a caller no level to misread.
///
/// [`NumericPresentation::EnclosingBucket`] is the way out: publishing the enclosing DECLARED
/// bucket makes a numeric column behave like a levelled one, at a cost in span width. It is why
/// that presentation exists.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnLevels {
    /// A levelled hierarchy — prefix or date. The published values are declared hierarchy nodes and
    /// the levels are comparable across partitions, across runs, and across releases.
    Levels {
        /// The coarsest level any published class sits at. `0` is the implicit top, `*`.
        coarsest: usize,
        /// The finest level any published class reached.
        finest: usize,
        /// Levels the hierarchy declares below the top. `finest == declared` means the hierarchy
        /// ran out before k did, and a deeper one would have released more detail.
        declared: usize,
    },
    /// A numeric column cut by Mondrian. There is no level here; these are the widths of the
    /// published spans, and they are reported so a caller can see how much resolution was lost
    /// without being handed a number that looks like a lattice position.
    Spans {
        /// The narrowest published span.
        narrowest: f64,
        /// The widest published span.
        widest: f64,
        /// How the spans reach the published table.
        presentation: NumericPresentation,
    },
}

/// Something the caller should know, which was not severe enough to refuse the run.
// No `Eq`: `BucketsDoNotCover` carries the `f64` that fell outside, and a float has no total
// equality. `PartialEq` is what the tests and `Vec::contains` need and is all that is true.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Warning {
    /// More quasi-identifiers than any k-anonymous generalisation can serve.
    ///
    /// Aggarwal (2005) showed that as the quasi-identifier count grows, the generalisation needed
    /// to reach k destroys the data: with enough attributes, every k-anonymous recoding of a
    /// realistic table generalises the great majority of its cells to the top of their hierarchy.
    /// The threshold is not sharp and the paper does not claim one; **ten** is the round number
    /// from its own experiments, past which the curve is already steep.
    ///
    /// The run still completes and its output is still k-anonymous. It is very likely also useless,
    /// and «silently returned rubbish» is the failure this warning exists to prevent.
    HighDimensionality {
        /// How many quasi-identifiers were declared.
        quasi_identifiers: usize,
        /// The threshold that was crossed.
        threshold: usize,
    },
    /// [`NumericPresentation::ObservedRange`] was chosen for a column, so its published spans have
    /// two real records' values as endpoints. See that variant's docs.
    ObservedRangeEndpoints {
        /// The column publishing real endpoints.
        column: String,
    },
    /// A value fell outside the declared bucket edges, so its class publishes an open-ended span
    /// (`(-inf, 18)` or `[65, inf)`) rather than a declared bucket.
    ///
    /// Not an error: an open-ended span is a correct generalisation and leaks no endpoint. It does
    /// mean the declared hierarchy does not describe the data it was applied to, which is worth
    /// knowing before the next release uses the same file.
    BucketsDoNotCover {
        /// The column whose buckets fell short.
        column: String,
        /// A value outside them.
        value: f64,
    },
    /// Nothing could be released: the table has fewer rows than k, or every row carried a missing
    /// quasi-identifier under [`crate::NullPolicy::Suppress`].
    EverythingSuppressed {
        /// Rows handed in.
        input_rows: usize,
        /// The k that could not be reached.
        k: usize,
    },
    /// The output verification withheld rows the partitioner had released. See
    /// [`Report::suppressed_by_safety_net`] — this is a bug in this crate, caught before it became
    /// a disclosure.
    SafetyNetFired {
        /// How many rows it withheld.
        rows: usize,
    },
}
