//! Mondrian strict multidimensional k-anonymity, over Arrow arrays, in a browser tab.
//!
//! Two halves, and they are separable on purpose.
//!
//! [`anonymize`] **derives** the generalisation: given the quasi-identifier columns, a k, and a
//! [`Hierarchy`] per column, it partitions the table and publishes generalised values.
//! [`verify::assess`] **checks** an already-published one: given the released quasi-identifier
//! columns and a k, it reports the achieved k, the equivalence-class sizes and the rows below the
//! bound. The second needs no hierarchy, no configuration and — this is the part that matters — no
//! sensitive column, so an auditor with a directory of Parquet files and no entitlement to the
//! diagnosis column can compute the compliance answer in full. See the [`verify`] module docs.
//!
//! # Why this crate exists
//!
//! Nothing on crates.io does this. The search was exhaustive; the one candidate, `diff-priv`, is
//! streaming microaggregation, was last published in 2022, and does not build for wasm32. OpenDP is
//! differential privacy — a different guarantee, not a stronger version of this one — and vendors
//! OpenSSL. ARX and Amnesia are the reference implementations of the generalisation literature and
//! are JVM and Python respectively. So this is a port, from the paper and from the behaviour of
//! `github.com/qiyuangong/Mondrian`, and it is deliberately one algorithm rather than a framework.
//!
//! # Which algorithm, and what was rejected
//!
//! **Mondrian strict**, from LeFevre, DeWitt and Ramakrishnan, *Mondrian Multidimensional
//! K-Anonymity*, ICDE 2006.
//!
//! Not Flash or Incognito. Those search a full generalisation lattice, which needs the lattice
//! built, a quality-metric framework to order it by, and predictive tagging to prune it — ARX is a
//! decade of work and this is not a reimplementation of it. Their payoff is *global* recoding, an
//! output where every cell of a column sits at one declared hierarchy level, and that payoff is
//! real: see the incomparability warning below.
//!
//! Not Datafly either, and this was the closer call, because Datafly *is* small, it *does* produce
//! hierarchy-level output, and global recoding is exactly what the incomparability problem wants.
//! It was rejected on utility: Datafly generalises a whole column at a time, so one dense region of
//! the table drags every other row up a level with it, and the published table is coarser — often
//! far coarser — than multidimensional recoding needs to be for the same k. Mondrian's own paper is
//! the measurement. The mitigation offered here instead is
//! [`NumericPresentation::EnclosingBucket`], which recovers comparable, declared values on numeric
//! columns without giving up local partitioning; a levelled column ([`hierarchy::Prefix`],
//! [`hierarchy::Date`]) already publishes declared hierarchy nodes and never had the problem.
//!
//! # What a caller must not assume
//!
//! **The output is not a hierarchy level, on numeric columns.** Mondrian is *local* recoding: each
//! partition's numeric span is computed from its own members, so two classes can publish `[29, 44]`
//! and `[31, 58]` — overlapping spans that nest in no lattice. You cannot join two outputs on such
//! a column, cannot compare «the level reached» across runs, and cannot roll one release up into
//! another. [`ColumnLevels`] encodes this in the type rather than in a comment: a numeric column
//! reports [`ColumnLevels::Spans`] and is given no level to misread. Use
//! [`NumericPresentation::EnclosingBucket`] where any of those three things matter.
//!
//! **Minimal generalisation is not the safest generalisation.** Wong, Fu, Wang and Pei, *Minimality
//! Attack in Privacy Preserving Data Publishing*, VLDB 2007, showed that publishing the table an
//! algorithm generalised *minimally* leaks: an adversary who knows the algorithm can reason
//! backwards from the level it stopped at — «it stopped here, so cutting once more would have left
//! a class below k, so that class had this shape» — and recover sensitive values that the k-anonymous
//! table appears to protect. Mondrian stops as soon as it can, so this crate produces exactly such a
//! table. It is not defended against, and defending against it is a different algorithm (m-invariance
//! and its descendants), not a flag. A caller publishing to an adversary who knows this crate is
//! running is holding a weaker guarantee than k suggests.
//!
//! **k-anonymity is not the whole of disclosure control.** It bounds re-identification, not
//! attribute disclosure: an equivalence class of 50 records that all carry the same diagnosis
//! discloses that diagnosis for all 50 while satisfying k = 50 comfortably. l-diversity and
//! t-closeness are the answers to that, they are properties of the SENSITIVE column, and this crate
//! never sees a sensitive column. Nothing here measures them.
//!
//! **A quasi-identifier not declared is not protected.** k is computed over the columns handed in.
//!
//! # High dimensionality
//!
//! Aggarwal (2005) showed the curse directly: as the quasi-identifier count grows, the
//! generalisation required to reach k destroys the data, and past roughly ten attributes essentially
//! every cell of a realistic table is generalised to the top of its hierarchy. The run still
//! completes and its output is still k-anonymous; it is also very likely useless.
//! [`Warning::HighDimensionality`] says so rather than letting the caller read a valid-looking
//! report over a table of asterisks.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use arrow_array::{ArrayRef, Int32Array, StringArray};
//! use fossil_kanon::{anonymize, Config, NullPolicy, QuasiIdentifier};
//! use fossil_kanon::hierarchy::{Hierarchy, Numeric, Prefix};
//!
//! let age: ArrayRef = Arc::new(Int32Array::from(vec![21, 24, 29, 33, 41, 47]));
//! let post: ArrayRef = Arc::new(StringArray::from(vec![
//!     "SW1A 1AA", "SW1A 2BB", "SW1P 3CC", "N1 4DD", "N1 5EE", "N7 6FF",
//! ]));
//!
//! let out = anonymize(
//!     &[
//!         QuasiIdentifier {
//!             name: "age".into(),
//!             values: age,
//!             // Declared buckets, so the published span names no record's real age.
//!             hierarchy: Hierarchy::Numeric(Numeric::bucketed(vec![18.0, 30.0, 45.0, 65.0])),
//!         },
//!         QuasiIdentifier {
//!             name: "postcode".into(),
//!             values: post,
//!             hierarchy: Hierarchy::Prefix(Prefix { lengths: vec![1, 2, 4] }),
//!         },
//!     ],
//!     &Config { k: 3, nulls: NullPolicy::Wildcard },
//! )
//! .unwrap();
//!
//! // The contract: what came back satisfies the k that was asked for.
//! assert!(out.assessment.satisfies_k);
//! assert!(out.report.achieved_k.unwrap() >= 3);
//! assert_eq!(out.report.input_rows, 6);
//! ```

// Every module below `lib` is private, so `pub(crate)` on the items they share is redundant to
// clippy and required by `unreachable_pub`, which the workspace turns on. The two lints want
// opposite spellings of the same fact; the rust lint is the one that catches a real leak.
#![allow(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace's `unreachable_pub`; see above"
)]
// «OpenDP», «DeWitt», «Ramakrishnan», «LeFevre» — proper nouns of the literature this crate ports,
// which `doc_markdown` reads as unbackticked identifiers. Backticking a surname is worse.
#![allow(
    clippy::doc_markdown,
    reason = "the prior art is cited by author name, and a surname is not code"
)]

pub mod hierarchy;
mod ingest;
mod mondrian;
mod report;
pub mod verify;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow_array::{ArrayRef, StringArray};

pub use hierarchy::{Hierarchy, NumericPresentation};
pub use report::{ColumnLevels, ColumnReport, Report, Warning};
pub use verify::{Assessment, EquivalenceClass, ReleasedColumn, WildcardPolicy};

use mondrian::DimState;
use verify::Cell;

/// Past this many quasi-identifiers, [`Warning::HighDimensionality`] is raised. Aggarwal's
/// experiments, rounded — the paper claims no sharp threshold and neither does this constant.
pub const HIGH_DIMENSIONALITY_THRESHOLD: usize = 10;

/// The token a wildcard is published as.
const WILDCARD: &str = "*";

/// One quasi-identifier column and how to generalise it.
#[derive(Debug, Clone)]
pub struct QuasiIdentifier {
    /// The column's name. Reaches error messages and [`ColumnReport::name`], and nothing else.
    pub name: String,
    /// The values. Integer, float, `Date32`/`Date64` or string, depending on the hierarchy —
    /// [`Error::UnsupportedType`] names the mismatch.
    pub values: ArrayRef,
    /// How this column is generalised.
    pub hierarchy: Hierarchy,
}

/// What a missing quasi-identifier means.
///
/// # It is a wildcard, and that is not the obvious implementation
///
/// A null quasi-identifier is not a value. In disclosure-control semantics it is compatible with
/// *every* value of the attribute, so a record carrying one is indistinguishable from the members
/// of every class it could have belonged to — its anonymity set is larger than its class, not
/// separate from it. The implementation that treats null as one more distinct value is the one that
/// comes naturally out of a group-by, and it is wrong in the unsafe direction: it collects the
/// records with missing postcodes into a small class of their own and reports it as k-anonymous
/// when it is a list of the people whose postcode nobody recorded.
///
/// So there is no such variant here, and the choice that remains is an explicit parameter with no
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullPolicy {
    /// Publish the missing value as `*` and count the record into the anonymity set of every class
    /// it is compatible with — the disclosure-control reading.
    ///
    /// A wildcard is placed on ONE side of a cut and pays towards k for that side only. It is
    /// tempting to count it towards both, since it is compatible with both, and that is unsound:
    /// the side it lands on is cut again on another dimension and takes the wildcard's cell with
    /// it, so the compatibility the *other* side was allowed on the strength of does not survive.
    /// See `mondrian::split_numeric`, and the nine-row counterexample the property test found.
    Wildcard,
    /// Withhold any record with a missing quasi-identifier before the search starts, and count it
    /// in [`Report::suppressed_by_null_policy`].
    ///
    /// Loses data and asserts nothing false. Choose it when a `*` in the released file would be
    /// read by its consumers as a value rather than as an absence.
    Suppress,
}

/// What to anonymise to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The anonymity bound. At least 2 — see [`Error::KTooSmall`].
    pub k: usize,
    /// What a missing quasi-identifier means. No default; see [`NullPolicy`].
    pub nulls: NullPolicy,
}

/// A generalised table.
#[derive(Debug, Clone)]
pub struct Anonymized {
    /// One `StringArray` per input quasi-identifier, in the input order and the input length. A
    /// suppressed row is null in every one of them.
    pub columns: Vec<ArrayRef>,
    /// Per input row, whether it reached the published table.
    pub released: Vec<bool>,
    /// The published table's equivalence classes, measured by [`verify`] over the published strings
    /// rather than counted by the partitioner.
    pub assessment: Assessment,
    /// What the run did.
    pub report: Report,
}

/// Everything that makes a run impossible rather than merely lossy.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No columns were declared. k over an empty quasi-identifier set is a property of nothing.
    #[error("no quasi-identifier columns were declared")]
    NoQuasiIdentifiers,
    /// `k < 2`.
    ///
    /// `k = 1` is not a weak anonymisation, it is the identity: every record is its own
    /// equivalence class and the output is the input. Asking for it is more likely a variable that
    /// should have held something else than an intention, so it is refused rather than served.
    #[error("k must be at least 2; k = 1 is the identity, not an anonymisation (got {k})")]
    KTooSmall {
        /// What was asked for.
        k: usize,
    },
    /// Columns of different lengths cannot be rows of one table.
    #[error("column `{column}` has {len} values; the table has {expected} rows")]
    LengthMismatch {
        /// The column.
        column: String,
        /// Its length.
        len: usize,
        /// The table's.
        expected: usize,
    },
    /// The Arrow type does not match what the declared hierarchy generalises.
    #[error("column `{column}` is {data_type}, which a `{hierarchy}` hierarchy cannot generalise")]
    UnsupportedType {
        /// The column.
        column: String,
        /// Its Arrow type.
        data_type: String,
        /// The hierarchy kind declared for it.
        hierarchy: &'static str,
    },
    /// An integer beyond 2^53, which `f64` cannot hold exactly.
    ///
    /// Refused rather than rounded: two source values that round to one `f64` would be published as
    /// one value and counted as one member of a class, so the reported k would be a number about a
    /// domain that is not the caller's.
    #[error("column `{column}` holds {value}, beyond the 2^53 an f64 represents exactly")]
    IntegerNotExact {
        /// The column.
        column: String,
        /// The offending value.
        value: i128,
    },
    /// A NaN in a numeric quasi-identifier.
    ///
    /// NaN is not a member of an ordered domain — it compares false against everything, so it sorts
    /// arbitrarily and would widen a published span to an interval whose endpoints are not the
    /// values it contains. It is not «missing» in the Arrow sense either; a caller who means
    /// unknown has a null bit and [`NullPolicy`] to say so with.
    #[error("column `{column}` holds NaN at row {row}; use a null to mean «missing»")]
    NotANumber {
        /// The column.
        column: String,
        /// The first offending row.
        row: usize,
    },
    /// A `Date64` whose millisecond count is not a day number `i32` can hold.
    #[error("column `{column}` holds a date at row {row} outside the representable range")]
    DateOutOfRange {
        /// The column.
        column: String,
        /// The offending row.
        row: usize,
    },
    /// [`NumericPresentation::EnclosingBucket`] with no buckets declared to enclose anything.
    #[error("column `{column}` publishes enclosing buckets but declares none")]
    BucketsRequired {
        /// The column.
        column: String,
    },
    /// Bucket edges that are not strictly ascending, or are not finite.
    #[error("column `{column}` declares bucket edges that are not strictly ascending and finite")]
    BucketsNotAscending {
        /// The column.
        column: String,
    },
    /// A hierarchy declaring no levels below the implicit top.
    #[error("column `{column}` declares a hierarchy with no levels")]
    EmptyHierarchy {
        /// The column.
        column: String,
    },
    /// Prefix lengths that are not strictly ascending, or that start at zero.
    ///
    /// Zero is the implicit top and is never declared; a hierarchy that declares it has an extra
    /// rung that publishes `*` twice.
    #[error("column `{column}` declares prefix lengths that are not strictly ascending from 1")]
    PrefixLengthsNotAscending {
        /// The column.
        column: String,
    },
    /// Date levels that do not run coarsest-first.
    #[error("column `{column}` declares date levels that do not run coarsest to finest")]
    DateLevelsNotDescending {
        /// The column.
        column: String,
    },
}

/// Derive the generalisation that reaches `k`.
///
/// See the crate docs for the algorithm, what it does not defend against, and what a caller must
/// not assume about its output.
///
/// # Errors
///
/// Every variant of [`Error`]: an empty or ragged column set, a `k` below 2, an Arrow type a
/// declared hierarchy cannot generalise, a value outside what the internal representation holds
/// exactly, or a malformed hierarchy.
pub fn anonymize(qis: &[QuasiIdentifier], config: &Config) -> Result<Anonymized, Error> {
    let Some(first) = qis.first() else {
        return Err(Error::NoQuasiIdentifiers);
    };
    if config.k < 2 {
        return Err(Error::KTooSmall { k: config.k });
    }
    let rows = first.values.len();
    let dims = ingest::resolve(qis, rows)?;
    let k = config.k;

    let mut warnings = Vec::new();
    if qis.len() > HIGH_DIMENSIONALITY_THRESHOLD {
        warnings.push(Warning::HighDimensionality {
            quasi_identifiers: qis.len(),
            threshold: HIGH_DIMENSIONALITY_THRESHOLD,
        });
    }
    for qi in qis {
        if let Hierarchy::Numeric(n) = &qi.hierarchy
            && n.presentation == NumericPresentation::ObservedRange
        {
            warnings.push(Warning::ObservedRangeEndpoints {
                column: qi.name.clone(),
            });
        }
    }

    // `NullPolicy::Suppress` acts before the search, so the partitioner never sees a wildcard and
    // the strict cut test degenerates to «both sides have k members», which is the textbook
    // statement of it.
    let live: Vec<u32> = (0..rows)
        .filter(|&r| match config.nulls {
            NullPolicy::Wildcard => true,
            NullPolicy::Suppress => !dims.iter().any(|d| d.values.is_null(r)),
        })
        .map(|r| u32::try_from(r).expect("a table with more than 4 billion rows is not in scope"))
        .collect();
    let suppressed_by_null_policy = rows - live.len();

    if live.len() < k {
        warnings.push(Warning::EverythingSuppressed {
            input_rows: rows,
            k,
        });
        return Ok(nothing_released(
            qis,
            &dims,
            rows,
            k,
            suppressed_by_null_policy,
            live.len(),
            warnings,
        ));
    }

    let partitions = mondrian::search(&dims, live, k);

    // Publish, and collect what the report needs while the partition is in hand.
    let mut released_rows: Vec<u32> = Vec::new();
    let mut cells: Vec<Vec<Cell>> = vec![Vec::new(); dims.len()];
    let mut spans: Vec<Vec<f64>> = vec![Vec::new(); dims.len()];
    let mut levels_seen: Vec<Vec<usize>> = vec![Vec::new(); dims.len()];
    let mut uncovered: HashMap<usize, f64> = HashMap::new();

    for part in &partitions {
        if part.rows.is_empty() {
            continue;
        }
        let published: Vec<mondrian::Published> = dims
            .iter()
            .enumerate()
            .map(|(d, dim)| mondrian::publish(dim, part, d))
            .collect();
        for (d, p) in published.iter().enumerate() {
            if let Some(s) = p.span {
                spans[d].push(s);
            }
            if let DimState::Level(l) = part.state[d] {
                levels_seen[d].push(l);
            }
            if let Some(v) = p.uncovered {
                uncovered.entry(d).or_insert(v);
            }
        }
        for &r in &part.rows {
            released_rows.push(r);
            for (d, dim) in dims.iter().enumerate() {
                // A record missing this quasi-identifier publishes `*` here, whatever its class
                // publishes. It is the one cell a class does not speak for.
                cells[d].push(if dim.values.is_null(r as usize) {
                    Cell::Wildcard
                } else {
                    published[d].cell.clone()
                });
            }
        }
    }

    for (d, v) in uncovered {
        warnings.push(Warning::BucketsDoNotCover {
            column: dims[d].name.clone(),
            value: v,
        });
    }

    // The safety net. Strict Mondrian cannot produce a class below k — a cut happens only when
    // every side already has k members — so this loop should never remove a row. It runs anyway,
    // because a privacy guarantee that holds conditionally on its author's proof being right is a
    // guarantee about the author. See `Report::suppressed_by_safety_net`.
    let mut suppressed_by_safety_net = 0usize;
    let assessment = loop {
        let a = verify::assess_cells(&cells, released_rows.len(), k, WILDCARD);
        if a.satisfies_k || released_rows.is_empty() {
            break a;
        }
        let failing: HashSet<&[String]> = a
            .classes
            .iter()
            .filter(|c| c.anonymity_set < k)
            .map(|c| c.values.as_slice())
            .collect();
        let keep: Vec<bool> = (0..released_rows.len())
            .map(|i| {
                let tuple: Vec<String> = cells.iter().map(|c| render(&c[i])).collect();
                !failing.contains(tuple.as_slice())
            })
            .collect();
        suppressed_by_safety_net += keep.iter().filter(|b| !**b).count();
        let mut it = keep.iter();
        released_rows.retain(|_| *it.next().expect("one flag per released row"));
        for c in &mut cells {
            let mut it = keep.iter();
            c.retain(|_| *it.next().expect("one flag per released row"));
        }
    };
    if suppressed_by_safety_net > 0 {
        warnings.push(Warning::SafetyNetFired {
            rows: suppressed_by_safety_net,
        });
    }
    if released_rows.is_empty()
        && !warnings.contains(&Warning::EverythingSuppressed {
            input_rows: rows,
            k,
        })
    {
        warnings.push(Warning::EverythingSuppressed {
            input_rows: rows,
            k,
        });
    }

    let mut released = vec![false; rows];
    for &r in &released_rows {
        released[r as usize] = true;
    }

    let columns = (0..dims.len())
        .map(|d| {
            let mut out: Vec<Option<String>> = vec![None; rows];
            for (i, &r) in released_rows.iter().enumerate() {
                out[r as usize] = Some(render(&cells[d][i]));
            }
            Arc::new(StringArray::from(out)) as ArrayRef
        })
        .collect();

    let columns_report = dims
        .iter()
        .enumerate()
        .map(|(d, dim)| ColumnReport {
            name: dim.name.clone(),
            levels: if dim.values.depth() == 0 {
                ColumnLevels::Spans {
                    narrowest: spans[d].iter().copied().fold(f64::INFINITY, f64::min),
                    widest: spans[d].iter().copied().fold(0.0, f64::max),
                    presentation: presentation_of(&qis[d].hierarchy),
                }
            } else {
                ColumnLevels::Levels {
                    coarsest: levels_seen[d].iter().copied().min().unwrap_or(0),
                    finest: levels_seen[d].iter().copied().max().unwrap_or(0),
                    declared: dim.values.depth(),
                }
            },
        })
        .collect();

    let report = Report {
        k,
        input_rows: rows,
        released_rows: released_rows.len(),
        suppressed_rows: rows - released_rows.len(),
        suppressed_by_null_policy,
        // Zero on this path by construction: the search only runs on a table that already clears k,
        // and the `live.len() < k` branch above returns before reaching it.
        suppressed_below_k: 0,
        suppressed_by_safety_net,
        equivalence_classes: assessment.classes.len(),
        achieved_k: assessment.achieved_k,
        smallest_exact_class: assessment.classes.iter().map(|c| c.exact_rows).min(),
        columns: columns_report,
        warnings,
    };

    Ok(Anonymized {
        columns,
        released,
        assessment,
        report,
    })
}

fn render(c: &Cell) -> String {
    match c {
        Cell::Wildcard => WILDCARD.to_owned(),
        Cell::Value(v) => v.clone(),
    }
}

const fn presentation_of(h: &Hierarchy) -> NumericPresentation {
    match h {
        Hierarchy::Numeric(n) => n.presentation,
        // Unreachable: only a numeric dimension reports spans.
        _ => NumericPresentation::ObservedRange,
    }
}

/// The empty release. Every column is all-null, every row is withheld, and the report says which of
/// the two reasons it was.
fn nothing_released(
    qis: &[QuasiIdentifier],
    dims: &[ingest::Dim],
    rows: usize,
    k: usize,
    suppressed_by_null_policy: usize,
    suppressed_below_k: usize,
    warnings: Vec<Warning>,
) -> Anonymized {
    let ncols = dims.len();
    let columns = (0..ncols)
        .map(|_| Arc::new(StringArray::from(vec![None::<String>; rows])) as ArrayRef)
        .collect();
    let columns_report = dims
        .iter()
        .enumerate()
        .map(|(d, dim)| ColumnReport {
            name: dim.name.clone(),
            levels: if dim.values.depth() == 0 {
                ColumnLevels::Spans {
                    narrowest: 0.0,
                    widest: 0.0,
                    presentation: presentation_of(&qis[d].hierarchy),
                }
            } else {
                ColumnLevels::Levels {
                    coarsest: 0,
                    finest: 0,
                    declared: dim.values.depth(),
                }
            },
        })
        .collect();
    Anonymized {
        columns,
        released: vec![false; rows],
        assessment: verify::assess_cells(&vec![Vec::new(); ncols], 0, k, WILDCARD),
        report: Report {
            k,
            input_rows: rows,
            released_rows: 0,
            suppressed_rows: suppressed_by_null_policy + suppressed_below_k,
            suppressed_by_null_policy,
            suppressed_below_k,
            suppressed_by_safety_net: 0,
            equivalence_classes: 0,
            achieved_k: None,
            smallest_exact_class: None,
            columns: columns_report,
            warnings,
        },
    }
}
