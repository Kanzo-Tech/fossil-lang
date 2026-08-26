//! Write-time verification of a declared privacy bound.
//!
//! # Where this runs, and why that is the whole design
//!
//! Between [`execute_graph`](crate::execute_graph) and
//! [`write_to_dir`](crate::GraphArData::write_to_dir): **after the corpus
//! exists as a value and before a single byte of it reaches a disk.** A refusal
//! here leaves nothing behind to leak.
//!
//! That position is forced. A corpus is files, and files have no chokepoint: a
//! recipient reads every column with any Parquet reader, and there is no read
//! path of ours in front of them to put a control on. The systems that do have
//! a chokepoint do not put the control on SQL either — Snowflake's dynamic data
//! masking and row access policies, `BigQuery`'s column-level ACLs and row-level
//! security, Databricks' equivalent, all apply inside the query planner
//! whatever the caller wrote. **The protection is that the bytes were never
//! written, not that a verb was withheld.**
//!
//! This is the *handed-over* corpus. A corpus that is **served** — the operator
//! keeps the files and the only access is through their engine — does have a
//! chokepoint and admits mechanisms this one cannot, at the price of a trusted
//! operator and a budget that runs out. Nothing here is built for that case and
//! nothing here assumes it away.
//!
//! # Three ways to get this wrong, each of them somebody else's measurement
//!
//! A `GROUP BY qi HAVING count(*) < k` is not this check. It is wrong in three
//! separate ways, and each is handled explicitly below rather than assumed away:
//!
//! 1. **Nulls.** SQL groups every `NULL` together; statistical disclosure
//!    control treats an absent quasi-identifier as a wildcard matching every
//!    value. Both readings are defensible and they disagree, so the reading is a
//!    **declared parameter** — [`AbsentQuasiIdentifier`] — and never a default
//!    this module picked.
//! 2. **Suppression.** k-anonymity in practice is generalisation *plus* a
//!    suppression limit. A verification that does not account for suppressed
//!    records verifies a different property, so the count and the budget are
//!    both measured and both published.
//! 3. **Scope.** The equivalence class is the **whole released population**. A
//!    corpus is tiles, which makes a per-tile aggregation the easiest wrong
//!    answer available here: it is more parallel, it looks the same, and every
//!    class it finds is a subset of a real one — so it reports a `k` that is too
//!    *small*, and a producer who clears the target anyway ships a bound that
//!    was never checked against the release. The aggregate below runs over one
//!    relation holding every batch of the type, and the population is asserted
//!    twice: against the count the manifest declares, and against the class
//!    sizes adding back up.
//!
//! # What this module does not do
//!
//! It does not *derive* anything. There is no generalisation search here — no
//! Mondrian, no Datafly, no recoding — and no suppression is performed: rows are
//! not dropped and cells are not blanked. It measures what the program produced
//! and refuses what does not clear the bar. **A verifier that also repairs is a
//! verifier whose failures nobody ever sees.**
//!
//! # The assumption this check cannot discharge
//!
//! Every vertex carries `subject`, a unique IRI, and k-anonymity is a bound on
//! the **quasi-identifier tuple** — not on the record. That is the standard
//! threat model: it stops a recipient linking a row to a person through an
//! external table keyed on the quasi-identifiers. It does **not** stop anything
//! if the subject IRI itself carries an identifier, because then the linkage
//! needs no quasi-identifiers at all. Nothing here can tell an opaque IRI from a
//! revealing one, and no guard downstream can either. It is stated in the
//! format's prose, in the guard's `cannotProve`, and here.

use std::collections::BTreeSet;
use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::Schema;
use datafusion::common::ScalarValue;
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use datafusion::functions_aggregate::expr_fn::{count, min, sum};
use datafusion::logical_expr::{Expr as DfExpr, lit};
use datafusion::prelude::{DataFrame, SessionContext, col, when};
use fossil_graph_schema::NodeType;
use fossil_policy::{AbsentQuasiIdentifier, Classification, PrivacyPolicy, ShapeRule};
use fossil_sinks::manifest::{KAnonymity, Privacy};

use crate::{GraphArData, VertexTable};

/// The cap on how many distinct quasi-identifier tuples the
/// [`AbsentQuasiIdentifier::Wildcard`] arithmetic will materialise.
///
/// Only that reading needs the class table in memory; the other two answer from
/// the engine in one pass. It is a cap and not a tuning knob. Reaching it means
/// the corpus has more than a million distinct tuples over its
/// quasi-identifiers — a corpus whose exact `k` is 1, asking the wildcard
/// reading to rescue it. Refusing is the honest answer, and the message names
/// the cheaper one.
const WILDCARD_CLASS_CAP: usize = 1 << 20;

/// Why a corpus was refused, or why the bound could not be established.
///
/// # Why no violating value is ever named
///
/// Every variant here names counts, column names and shape labels, and **never
/// a cell**. A diagnostic about a privacy violation that prints the violating
/// row has moved the disclosure from the artifact into the log, the terminal
/// scrollback and whatever CI captured it — and unlike the corpus, none of those
/// was ever going to be checked by a guard. The smallest class is reported by
/// its *size*.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    /// The policy is silent about an output type the program writes.
    #[error(
        "the policy declares a k-anonymity bound and says nothing about the output type `{shape}`. \
         Every type a program writes has to be considered: give `{shape}` a rule, with an empty \
         quasi-identifier set if it carries nothing to protect"
    )]
    ShapeNotCovered {
        /// The uncovered vertex type.
        shape: String,
    },

    /// A column the policy classifies as identifying on its own is published.
    #[error(
        "`{shape}.{column}` is classified as a direct identifier and the corpus publishes it. \
         A k-anonymity bound over a release carrying a direct identifier is not a bound — the \
         quasi-identifiers stop mattering once one column names the person"
    )]
    DirectIdentifierPublished {
        /// The vertex type.
        shape: String,
        /// The offending column.
        column: String,
    },

    /// A prohibited column is published.
    #[error(
        "the policy prohibits publishing `{shape}.{column}` and the corpus carries that column"
    )]
    ProhibitedPublished {
        /// The vertex type.
        shape: String,
        /// The offending column.
        column: String,
    },

    /// The bound was measured and is not met.
    #[error(
        "`{shape}` reaches k={reached} over {population} record(s) on ({quasi_identifiers}), and \
         the policy asks for k={k}: {below} equivalence class(es) hold fewer than {k}, \
         {at_risk} record(s) between them"
    )]
    BoundNotReached {
        /// The vertex type.
        shape: String,
        /// The tuple the bound was measured over.
        quasi_identifiers: String,
        /// What the policy asked for.
        k: u64,
        /// The smallest class found.
        reached: u64,
        /// Records measured.
        population: u64,
        /// How many classes are under `k` — because "one class of four" and
        /// "nine thousand classes of one" are the same `reached` and completely
        /// different corpora.
        below: u64,
        /// The records in those classes.
        at_risk: u64,
    },

    /// More was suppressed than the policy allows.
    #[error(
        "`{shape}` suppresses {suppressed} of {population} record(s), which is {spent} parts per \
         million against a budget of {budget}. k-anonymity is generalisation plus a suppression \
         limit, and this release is over the limit"
    )]
    BudgetExceeded {
        /// The vertex type.
        shape: String,
        /// Records not certified.
        suppressed: u64,
        /// Records measured.
        population: u64,
        /// What was spent, in parts per million.
        spent: u64,
        /// What was allowed.
        budget: u64,
    },

    /// The aggregation did not span the release.
    #[error(
        "the check over `{shape}` spanned {counted} record(s) and the type has {declared}. The \
         equivalence class is the whole released population, so a check that did not span it has \
         measured something else"
    )]
    ScopeNotSpanned {
        /// The vertex type.
        shape: String,
        /// What the check saw.
        counted: u64,
        /// What the corpus holds.
        declared: u64,
    },

    /// The wildcard reading was asked for more classes than it will hold.
    #[error(
        "`{shape}` has {classes} distinct quasi-identifier tuples, past the {cap} this reading \
         will materialise. `wildcard` is the only reading that needs them in memory, and it can \
         only ever report a LARGER k than `value` does — so a corpus it would rescue is one whose \
         exact k is 1. Declare `value`"
    )]
    TooManyClasses {
        /// The vertex type.
        shape: String,
        /// How many were found.
        classes: u64,
        /// The cap.
        cap: usize,
    },

    /// The engine failed.
    #[error("verifying the privacy bound: {0}")]
    Engine(#[from] DataFusionError),
}

/// What one type's population measured to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    /// The vertex type this describes.
    pub shape: String,
    /// The quasi-identifier columns actually used, as they appear in the corpus.
    ///
    /// **Resolved against the bytes, not taken from the policy.** A policy may
    /// name a quasi-identifier this release does not publish, and the tuple the
    /// bound is over is the one that exists. Fewer columns means coarser classes
    /// — a stronger position, not a weaker one — but it is a different claim
    /// from the policy's, so what gets published on the manifest is this list.
    pub columns: Vec<String>,
    /// Every record of the type. Equal to the type's `vertex_count`.
    pub population: u64,
    /// Records not certified by a class, under
    /// [`AbsentQuasiIdentifier::Suppress`] only.
    pub suppressed: u64,
    /// The smallest equivalence class, under the declared reading.
    pub reached: u64,
    /// Classes holding fewer than `k` records.
    pub below: u64,
    /// The records in those classes.
    pub at_risk: u64,
}

/// Verify `graph` against `policy`, returning the bound to seal into the
/// manifest.
///
/// # Errors
/// [`Refusal`] — the bound was not reached, the policy does not cover the
/// corpus, a prohibited or identifying column is published, the budget is
/// overspent, or the aggregation did not span the release.
pub async fn verify(policy: &PrivacyPolicy, graph: &GraphArData) -> Result<Privacy, Refusal> {
    let mut probe = fossil_mem_probe::Probe::new("verify_privacy");

    let mut measurements = Vec::with_capacity(graph.schema.nodes.len());
    for node in &graph.schema.nodes {
        // Every output type has to be CONSIDERED, and «considered» means the
        // policy names it. A policy covering three of four types and silent
        // about the fourth is the most likely way for this whole mechanism to
        // pass while protecting nothing, and silence is indistinguishable from
        // a judgement that there was nothing to protect. So it is refused, and
        // the escape is one line: a rule with an empty set.
        let rule = policy
            .shape(&node.label)
            .ok_or_else(|| Refusal::ShapeNotCovered {
                shape: node.label.clone(),
            })?;

        let Some(table) = graph.vertices.iter().find(|v| v.label == node.label) else {
            // A declared type that materialised nothing. `manifest()` writes it
            // as `vertex_count: 0`; a population of zero has no class to measure
            // and is not evidence of anything either way.
            continue;
        };

        let published = published_columns(table);
        for column in &published {
            let iri = node
                .properties
                .iter()
                .find(|p| &p.name == column)
                .and_then(|p| p.iri.as_deref());
            if rule.is_prohibited(iri, column) {
                return Err(Refusal::ProhibitedPublished {
                    shape: node.label.clone(),
                    column: column.clone(),
                });
            }
            if rule.classify(iri, column) == Classification::DirectIdentifier {
                return Err(Refusal::DirectIdentifierPublished {
                    shape: node.label.clone(),
                    column: column.clone(),
                });
            }
        }

        let columns = resolve_quasi_identifiers(rule, node, &published);
        let measured = measure(
            &node.label,
            table,
            &columns,
            policy.absent_quasi_identifier,
            policy.k,
        )
        .await?;

        // Scope, asserted rather than assumed. The same sum `manifest()` writes
        // as `vertex_count` — so this is the check that the aggregate saw the
        // release and not a tile of it.
        let declared = rows_of(table);
        if measured.population != declared {
            return Err(Refusal::ScopeNotSpanned {
                shape: node.label.clone(),
                counted: measured.population,
                declared,
            });
        }

        if measured.reached < policy.k {
            return Err(Refusal::BoundNotReached {
                shape: node.label.clone(),
                quasi_identifiers: measured.columns.join(", "),
                k: policy.k,
                reached: measured.reached,
                population: measured.population,
                below: measured.below,
                at_risk: measured.at_risk,
            });
        }

        // Integer arithmetic on both sides, which is the whole reason the budget
        // is parts per million and not a fraction.
        if measured.suppressed.saturating_mul(1_000_000)
            > measured
                .population
                .saturating_mul(policy.suppression_budget_ppm)
        {
            let spent = if measured.population == 0 {
                0
            } else {
                measured.suppressed.saturating_mul(1_000_000) / measured.population
            };
            return Err(Refusal::BudgetExceeded {
                shape: node.label.clone(),
                suppressed: measured.suppressed,
                population: measured.population,
                spent,
                budget: policy.suppression_budget_ppm,
            });
        }

        measurements.push(measured);
    }
    probe.mark(&format!("{} type(s) measured", measurements.len()));
    probe.finish();

    Ok(seal(policy, &measurements))
}

/// Fold the per-type measurements into the one corpus-level bound the manifest
/// carries.
///
/// `reached` is the **minimum** over types and the population is the **sum**:
/// the field says what the release guarantees, and a release is only as
/// anonymous as its weakest type. A corpus with nothing measured reaches its own
/// population, for the reason [`measure`] gives about the empty tuple.
fn seal(policy: &PrivacyPolicy, measurements: &[Measured]) -> Privacy {
    let population = measurements.iter().map(|m| m.population).sum();
    let suppressed = measurements.iter().map(|m| m.suppressed).sum();
    let reached = measurements
        .iter()
        .map(|m| m.reached)
        .min()
        .unwrap_or(population);
    // Sorted, so the string is a function of the corpus rather than of the order
    // the policy happened to list things in. The order means nothing to the
    // arithmetic; it means everything to two runs of one program producing the
    // same bytes.
    let mut names: Vec<String> = measurements
        .iter()
        .flat_map(|m| m.columns.iter().map(|c| format!("{}.{c}", m.shape)))
        .collect();
    names.sort();

    Privacy::KAnonymity(KAnonymity {
        k: policy.k,
        reached,
        absent_quasi_identifier: policy.absent_quasi_identifier,
        population,
        suppressed,
        suppression_budget_ppm: policy.suppression_budget_ppm,
        quasi_identifiers: names.join(" "),
        policy: policy.uid.clone(),
        profile: policy.profile.clone(),
    })
}

/// Every row of every batch — the same sum `manifest()` writes as
/// `vertex_count`.
fn rows_of(table: &VertexTable) -> u64 {
    table
        .batches
        .iter()
        .map(RecordBatch::num_rows)
        .sum::<usize>() as u64
}

/// The columns the corpus actually carries for this type, off the Arrow schema
/// rather than off the graph-schema — the bytes are what a recipient gets.
fn published_columns(table: &VertexTable) -> Vec<String> {
    table.batches.first().map_or_else(Vec::new, |b| {
        b.schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    })
}

/// The quasi-identifier tuple this release is actually measured over.
///
/// The policy's declared set intersected with what is published — or, when the
/// policy declared no explicit set, every published predicate it classifies as a
/// quasi-identifier. Sorted, for [`seal`]'s determinism reason.
fn resolve_quasi_identifiers(
    rule: &ShapeRule,
    node: &NodeType,
    published: &[String],
) -> Vec<String> {
    let iri_of = |name: &str| {
        node.properties
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.iri.clone())
    };
    published
        .iter()
        .filter(|name| {
            let iri = iri_of(name);
            let iri = iri.as_deref();
            if rule.quasi_identifiers.is_empty() {
                rule.classify(iri, name) == Classification::QuasiIdentifier
            } else {
                rule.quasi_identifiers
                    .iter()
                    .any(|key| iri == Some(key.as_str()) || key == *name)
            }
        })
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Measure one type's smallest equivalence class over `columns`, under the
/// declared reading of an absent value.
///
/// **One relation, every batch.** `MemTable::try_new` is handed the whole
/// `Vec<RecordBatch>` as one partition list and the aggregate is unpartitioned,
/// so the classes are classes of the release. This is where the tile mistake
/// would live if it lived anywhere.
async fn measure(
    shape: &str,
    table: &VertexTable,
    columns: &[String],
    absent: AbsentQuasiIdentifier,
    k: u64,
) -> Result<Measured, Refusal> {
    let ctx = SessionContext::new();
    let schema = table
        .batches
        .first()
        .map_or_else(|| Arc::new(Schema::empty()), RecordBatch::schema);
    // Cloning a `RecordBatch` clones `Arc`s, not buffers: this registration is a
    // second VIEW of the corpus already in memory, not a second copy of it. That
    // matters — the whole graph is resident at this point by construction, and
    // the encode that follows is the other peak.
    let mem = MemTable::try_new(schema, vec![table.batches.clone()])?;
    ctx.register_table("release", Arc::new(mem))?;
    let df = ctx.table("release").await?;

    let population = rows_of(table);

    // «This record has an absent quasi-identifier». An empty tuple has none,
    // which is why the fold falls back to false rather than true: a type with no
    // quasi-identifiers suppresses nothing and forms one class holding everyone.
    let any_null = columns
        .iter()
        .map(|c| col(c).is_null())
        .reduce(DfExpr::or)
        .unwrap_or_else(|| lit(false));

    let (classified, suppressed) =
        if absent == AbsentQuasiIdentifier::Suppress && !columns.is_empty() {
            let counted = one_row(
                df.clone()
                    .filter(any_null.clone())?
                    .aggregate(vec![], vec![count(lit(1)).alias("n")])?,
            )
            .await?;
            (
                df.filter(!any_null)?,
                counted.first().copied().flatten().unwrap_or(0),
            )
        } else {
            (df, 0)
        };

    // The class table: one row per distinct tuple, with its size. Zero grouping
    // columns is NOT a special case — it is one group holding the whole
    // population, which is exactly the right answer for a type publishing no
    // quasi-identifier at all.
    let classes = classified.aggregate(
        columns.iter().map(String::as_str).map(col).collect(),
        vec![count(lit(1)).alias("n")],
    )?;

    let (reached, counted, below, at_risk) =
        if absent == AbsentQuasiIdentifier::Wildcard && !columns.is_empty() {
            wildcard(shape, classes, columns.len(), k).await?
        } else {
            // Four numbers, one pass over the class table: the smallest class,
            // the records in all of them, and the two that describe the failure
            // if there is one.
            let under = || col("n").lt(lit(k as i64));
            let summary = classes.aggregate(
                vec![],
                vec![
                    min(col("n")).alias("smallest"),
                    sum(col("n")).alias("total"),
                    sum(when(under(), lit(1_i64)).otherwise(lit(0_i64))?).alias("below"),
                    sum(when(under(), col("n")).otherwise(lit(0_i64))?).alias("at_risk"),
                ],
            )?;
            let row = one_row(summary).await?;
            let at = |i: usize| row.get(i).copied().flatten().unwrap_or(0);
            (
                // `min` over no rows is NULL, and no rows means no certified
                // records — a population of zero, whose smallest class is
                // vacuously the whole of it.
                row.first()
                    .copied()
                    .flatten()
                    .unwrap_or(population.saturating_sub(suppressed)),
                at(1),
                at(2),
                at(3),
            )
        };

    // Every certified record is in exactly one class, so the class sizes have to
    // add back up to the certified population. This is the second half of the
    // scope assertion and it catches what the first half cannot: an aggregate
    // that ran over a subset still agrees with itself.
    if counted + suppressed != population {
        return Err(Refusal::ScopeNotSpanned {
            shape: shape.to_string(),
            counted: counted + suppressed,
            declared: population,
        });
    }

    Ok(Measured {
        shape: shape.to_string(),
        columns: columns.to_vec(),
        population,
        suppressed,
        reached,
        below,
        at_risk,
    })
}

/// The smallest **wildcard** frequency count, the certified population, and the
/// two failure numbers.
///
/// Under [`AbsentQuasiIdentifier::Wildcard`] a record's class is every record
/// agreeing with it wherever both are non-`NULL`, so a record with an absent
/// value borrows the size of every class it could belong to:
///
/// ```text
/// f(r) = Σ { |c| : c compatible with r }
/// ```
///
/// # The shortcut that is only valid on one side, and was taken on both
///
/// A class holding no `NULL` is compatible with `r` only by agreeing with `r`
/// everywhere `r` is non-`NULL` — so when **`r` itself has no `NULL`**, the only
/// such class is `r`'s own, and the sum collapses to
/// `|r's class| + Σ over null-bearing c ≠ r`. That is a real saving and it is
/// what makes the common case cheap.
///
/// It is **false when `r` has a `NULL`**, because then `r` is compatible with
/// every class agreeing on the columns `r` does have — including plenty of
/// classes that hold no `NULL` at all. Applying it on that side too is a bug
/// this function shipped with for exactly one test run: on six records at
/// `(1980, SW1)` and four at `(1980, NULL)` it reported `k = 4`, having given
/// the four absent postcodes credit for nobody but themselves, when the whole
/// premise of the wildcard reading is that those four could be any postcode and
/// are therefore hiding among the six. The answer is 10. The two branches below
/// are that asymmetry, and the test that caught it is
/// `the_two_null_readings_disagree_on_one_file`.
///
/// Cost either way is `|classes| × |null-bearing classes|`, so a corpus with no
/// missing values pays for the collect and nothing else.
async fn wildcard(
    shape: &str,
    classes: DataFrame,
    width: usize,
    k: u64,
) -> Result<(u64, u64, u64, u64), Refusal> {
    let batches = classes.collect().await?;
    let total: usize = batches.iter().map(RecordBatch::num_rows).sum();
    if total > WILDCARD_CLASS_CAP {
        return Err(Refusal::TooManyClasses {
            shape: shape.to_string(),
            classes: total as u64,
            cap: WILDCARD_CLASS_CAP,
        });
    }

    // The keys as `ScalarValue`s — the GROUP BY keys the engine itself produced,
    // so two records of one class are equal here by construction and the
    // arithmetic below only ever asks whether two keys agree.
    let mut table: Vec<(Vec<ScalarValue>, u64)> = Vec::with_capacity(total);
    for batch in &batches {
        for row in 0..batch.num_rows() {
            let key = (0..width)
                .map(|c| ScalarValue::try_from_array(batch.column(c), row))
                .collect::<Result<Vec<_>, _>>()?;
            let size = ScalarValue::try_from_array(batch.column(width), row)?;
            table.push((key, as_u64(&size)));
        }
    }

    let certified: u64 = table.iter().map(|(_, n)| n).sum();
    let with_null: Vec<&(Vec<ScalarValue>, u64)> = table
        .iter()
        .filter(|(key, _)| key.iter().any(ScalarValue::is_null))
        .collect();

    let mut smallest = u64::MAX;
    let mut below = 0;
    let mut at_risk = 0;
    let compatible = |a: &[ScalarValue], b: &[ScalarValue]| {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.is_null() || y.is_null() || x == y)
    };
    for (key, size) in &table {
        let f = if key.iter().any(ScalarValue::is_null) {
            // `key` is compatible with classes that hold no `NULL`, so the sum
            // is over everything and `key`'s own class is one of the terms.
            table
                .iter()
                .filter(|(other, _)| compatible(key, other))
                .map(|(_, n)| n)
                .sum()
        } else {
            // `key` agrees with a `NULL`-free class only by being it, so that
            // term is `size` and the rest can only come from the null-bearing
            // ones.
            *size
                + with_null
                    .iter()
                    .filter(|(other, _)| other != key && compatible(key, other))
                    .map(|(_, n)| n)
                    .sum::<u64>()
        };
        smallest = smallest.min(f);
        if f < k {
            below += 1;
            at_risk += size;
        }
    }

    Ok((
        if table.is_empty() { 0 } else { smallest },
        certified,
        below,
        at_risk,
    ))
}

/// Collect a one-row frame into its columns, as `u64`s. `None` for a `NULL`,
/// which is what `min` and `sum` over no rows are.
async fn one_row(df: DataFrame) -> Result<Vec<Option<u64>>, Refusal> {
    let batches = df.collect().await?;
    let Some(batch) = batches.first().filter(|b| b.num_rows() > 0) else {
        return Ok(Vec::new());
    };
    (0..batch.num_columns())
        .map(|c| {
            let v = ScalarValue::try_from_array(batch.column(c), 0)?;
            Ok(if v.is_null() { None } else { Some(as_u64(&v)) })
        })
        .collect::<Result<Vec<_>, DataFusionError>>()
        .map_err(Refusal::Engine)
}

/// A counting aggregate's value as a `u64`. `count` and `sum` come back signed
/// here, and a negative count is not a thing a count can be.
fn as_u64(v: &ScalarValue) -> u64 {
    match v {
        ScalarValue::Int64(Some(n)) => u64::try_from(*n).unwrap_or(0),
        ScalarValue::UInt64(Some(n)) => *n,
        ScalarValue::Int32(Some(n)) => u64::try_from(*n).unwrap_or(0),
        ScalarValue::UInt32(Some(n)) => u64::from(*n),
        _ => 0,
    }
}
