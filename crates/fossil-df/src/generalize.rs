//! Deriving the generalisation a declared k-anonymity bound needs.
//!
//! # The half that was missing
//!
//! [`crate::privacy`] verifies a bound and refuses a release that misses it.
//! `fossil-kanon` derives a generalisation. For a while those two shipped with
//! nothing between them, and the consequence was not a gap in a feature list: it
//! was that **the verification could only ever refuse.** Nothing in the system
//! could produce a generalised column, so a release passed exactly when its
//! source data happened to be k-anonymous already, and a policy author who
//! declared a bound their data did not meet had no move available except to
//! lower the bound. This module is the edge between them.
//!
//! # Where it runs, and why it is not inside the verifier
//!
//! Immediately before [`crate::privacy::verify`], over the same in-memory Arrow
//! batches, in [`crate::run_to_dir`] — so the order is
//! `execute_graph` → **derive** → `verify` → `write_to_dir`. That seam is forced
//! by the same argument that put verification there: a corpus is files and files
//! have no chokepoint, so every control has to act before a byte reaches a disk.
//! Deriving after the write would mean rewriting a corpus that had already
//! escaped, and deriving before `execute_graph` would mean generalising a table
//! that does not exist yet.
//!
//! It is a **separate module and a separate call**, and that is not tidiness.
//! `privacy.rs` says, in as many words, that a verifier that also repairs is a
//! verifier whose failures nobody ever sees. That property is kept literally:
//! [`apply`] mutates the corpus and returns no measurement, [`crate::privacy::verify`]
//! measures the corpus and performs no repair, and neither one is reachable from
//! the other.
//!
//! # Verification does not trust this module, by construction
//!
//! `fossil-kanon` already re-reads its own output through `verify::assess`
//! before returning it. That is its guarantee about itself and it is not this
//! corpus's guarantee, because a deriver checking its own work with its own code
//! establishes only that the code agrees with itself.
//!
//! So the release is measured **twice, by two implementations that share no
//! code**: once inside `fossil-kanon`, over in-memory `Cell` tuples, by an
//! exact-match tabulation; and once by [`crate::privacy::verify`], over the
//! written Arrow columns, by a DataFusion aggregate that knows nothing about
//! this module and is handed nothing by it. The second one is the one the
//! manifest records. Nothing flows from [`apply`] into `verify` — not the
//! achieved k, not the class count, not the column list, not a hint that
//! derivation happened at all — and [`Derived`] carries only the strings the
//! manifest prints, which no check consults. **If those two ever agreed by
//! construction rather than by measurement, the guarantee would be circular**,
//! and the shape of the call in `run_to_dir` is what stops it.
//!
//! A consequence worth stating plainly: this module is allowed to fail at its
//! job. A derivation that does not reach the bound produces a refusal from the
//! verifier, exactly as an ungeneralised corpus that misses the bound does. It
//! is best-effort; the verifier is the guarantee.
//!
//! # Minimality, and what is actually done about it
//!
//! Wong, Fu, Wang and Pei, *Minimality Attack in Privacy Preserving Data
//! Publishing*, VLDB 2007: publishing the table an algorithm generalised
//! *minimally* leaks. An adversary who knows the algorithm reasons backwards
//! from where it stopped — «it stopped here, so one more cut would have left a
//! class below k, so that class had this shape» — and recovers values the
//! k-anonymous table appears to protect. Mondrian stops as soon as it can, so
//! `fossil-kanon` produces exactly such a table and says so.
//!
//! This module does **not** defend against it. Defending means m-invariance or
//! one of its descendants, which is a different algorithm and not a flag. What
//! it does instead is three things, none of them a defence and all of them
//! visible:
//!
//! 1. **It refuses [`NumericPresentation::ObservedRange`].** Mondrian's own
//!    numeric output is `[min, max]` over the partition, and both endpoints are
//!    values a specific record actually holds — two attributes released in full,
//!    in a column whose whole purpose was to be generalised. Only
//!    [`NumericPresentation::EnclosingBucket`] is accepted here, so every
//!    published cell in the corpus is a node of a **declared** hierarchy.
//! 2. That has a real effect on this attack, and it is worth being precise
//!    about how much. A declared bucket is a function of the hierarchy and not
//!    of the partition, so two partitions Mondrian cut apart at a median
//!    publish the same bucket and merge. The published table is therefore
//!    **coarser than Mondrian's minimal output**, and the cut position — which
//!    is what the minimality inference is about — is not in the released bytes
//!    to reason back from. It is a mitigation, it is partial, and it is not a
//!    reason to describe the release as defended.
//! 3. **It is written on the artifact.** The manifest's
//!    `KAnonymity::generalization` names every generalised column and the
//!    hierarchy levels it reached, so a recipient reads what was done rather
//!    than inferring it. A producer publishing to an adversary who knows this
//!    code is running holds a weaker guarantee than `k` suggests, and the corpus
//!    now says enough for that to be somebody's decision rather than a surprise.
//!
//! The knob a producer actually has is `fossil:anonymityK` itself: asking for a
//! larger `k` than the release must clear produces a table that is not minimal
//! for the bound it claims, with the number chosen visible on the artifact. That
//! is why no seventh profile term was minted for a margin — see
//! `fossil_policy::profile`.
//!
//! # The three traps this must not reintroduce
//!
//! `privacy.rs` handles nulls-as-wildcards, the suppression budget, and the
//! equivalence class being the whole release rather than a tile. A deriver is
//! in a position to silently undo all three, so:
//!
//! - **Scope.** Every batch of a type is concatenated into ONE table before
//!   `anonymize` sees it, and sliced back into batches of exactly the original
//!   lengths afterwards. A deriver that partitioned per batch would generalise
//!   each tile against its own neighbours, reach `k` within every tile, and
//!   produce a release whose real classes are smaller than any tile's — the tile
//!   trap, arrived at from the other side. The corpus is tiles and the class is
//!   the release.
//! - **Nulls.** A cell that was null in the input is published null, whatever
//!   `anonymize` rendered it as. `fossil-kanon` publishes `*` for a missing
//!   quasi-identifier under its wildcard policy, and writing that string into
//!   the corpus would destroy the verifier's ability to see an absence at all:
//!   `AbsentQuasiIdentifier::Suppress` counts nulls, and against a column of
//!   `*`s it would count zero and the budget check would pass vacuously. The
//!   input null mask is re-applied for exactly that reason.
//! - **Suppression.** Rows are never dropped. A row `anonymize` withheld keeps
//!   its place in the table with null quasi-identifiers, so the population the
//!   verifier measures is the population the program produced, the scope
//!   assertion still has something to assert against, and a withheld row is
//!   charged to the suppression budget under the reading that charges for one.
//!   A deriver that deleted rows would shrink `vertex_count` to match its own
//!   output and no check downstream could tell.

use std::collections::BTreeSet;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, RecordBatch, StringArray};
use datafusion::arrow::compute::concat;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use fossil_graph_schema::{NodeType, Primitive};
use fossil_kanon::hierarchy::Hierarchy;
use fossil_kanon::{ColumnLevels, Config, NullPolicy, NumericPresentation, QuasiIdentifier};
use fossil_policy::{AbsentQuasiIdentifier, PrivacyPolicy};
use fossil_sinks::manifest::Privacy;

use crate::{GraphArData, VertexTable};

/// Why a generalisation could not be derived.
///
/// Every variant here names a column, a shape or a hierarchy kind, and — the
/// same rule [`crate::privacy::Refusal`] follows and for the same reason —
/// **never a cell**. A diagnostic about a privacy control that printed the data
/// it was protecting would have moved the disclosure into the terminal, the log
/// and whatever CI captured it, none of which a guard will ever read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The hierarchy in the policy is not one this crate can read.
    ///
    /// `fossil-policy` checks the `kind` and stops there — it is a leaf and does
    /// not link the anonymiser — so the levels inside are first read here.
    #[error(
        "the policy generalises `{shape}.{column}` by a hierarchy this writer cannot read: {reason}"
    )]
    Malformed {
        /// The vertex type.
        shape: String,
        /// The column.
        column: String,
        /// What serde said.
        reason: String,
    },

    /// [`NumericPresentation::ObservedRange`] was declared.
    #[error(
        "the policy generalises `{shape}.{column}` with `observed_range`, and the writer publishes \
         only declared hierarchy nodes. An observed range is `[min, max]` over the partition, and \
         both endpoints are values a real record holds — two attributes released in full, in the \
         column that was being generalised. Declare `enclosing_bucket` with the buckets it should \
         publish"
    )]
    ObservedRangeRefused {
        /// The vertex type.
        shape: String,
        /// The column.
        column: String,
    },

    /// Some but not all of a shape's published quasi-identifiers are generalised.
    ///
    /// The one constraint that makes derivation and verification talk about the
    /// same thing; see its message.
    #[error(
        "`{shape}` generalises {generalised} of its {total} published quasi-identifiers and leaves \
         ({missing}) alone. The bound is over the WHOLE tuple: the deriver would reach k over the \
         columns it was handed while the verifier measures the columns that were published, and \
         classes over more columns are never larger — so a partial generalisation optimises for a \
         tuple nobody checks. Declare a `fossil:generalization` for every quasi-identifier of \
         `{shape}`, or for none of them"
    )]
    PartiallyCovered {
        /// The vertex type.
        shape: String,
        /// How many have a hierarchy.
        generalised: usize,
        /// How many quasi-identifiers the release publishes for this type.
        total: usize,
        /// The ones that do not, named.
        missing: String,
    },

    /// The anonymiser refused the input.
    #[error("deriving the generalisation for `{shape}`: {source}")]
    Kanon {
        /// The vertex type.
        shape: String,
        /// What `fossil-kanon` said — a column name and a reason, never a value.
        #[source]
        source: fossil_kanon::Error,
    },

    /// Arrow could not concatenate or slice a column.
    #[error("assembling `{shape}.{column}` across the release: {source}")]
    Arrow {
        /// The vertex type.
        shape: String,
        /// The column.
        column: String,
        /// The kernel's error.
        #[source]
        source: datafusion::arrow::error::ArrowError,
    },
}

/// What was generalised, in the spelling the manifest prints.
///
/// **This is the only thing that leaves this module, and no check reads it.**
/// It holds no k, no class count and no verdict, which is what keeps
/// [`crate::privacy::verify`] an independent measurement rather than a second
/// opinion informed by the first — see the module docs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Derived {
    /// One `<Type>.<column>@<levels>` token per generalised column, sorted, for
    /// the reason [`crate::privacy`]'s `seal` sorts the quasi-identifier names:
    /// two runs of one program over one corpus must produce one string.
    tokens: BTreeSet<String>,
}

impl Derived {
    /// The manifest's `generalization` scalar.
    ///
    /// `none` — a word, not an empty string — when nothing was derived. An empty
    /// string is what a manifest written before the field existed deserialises
    /// to, and «no hierarchy was declared» has to be distinguishable from «this
    /// producer could not have told you», which is the same distinction
    /// [`Privacy::Undeclared`] exists to draw one level up.
    #[must_use]
    pub fn render(&self) -> String {
        if self.tokens.is_empty() {
            "none".to_string()
        } else {
            self.tokens.iter().cloned().collect::<Vec<_>>().join(" ")
        }
    }
}

/// Derive and apply the generalisation the policy declares, in place.
///
/// Returns what to print on the manifest and **nothing a check consults**. The
/// corpus that comes back is the corpus [`crate::privacy::verify`] then measures
/// as if a stranger had handed it the bytes.
///
/// A shape the policy does not cover, or one that declares no hierarchy, is left
/// exactly as the program produced it: this function is not where an uncovered
/// shape is caught, because refusing is the verifier's job and doing it twice in
/// two places is how the two messages start disagreeing.
///
/// # Errors
/// [`Error`] — a hierarchy this writer cannot read, an observed-range
/// presentation, a partially covered shape, an Arrow type no declared hierarchy
/// generalises, or an Arrow kernel failure.
pub fn apply(policy: &PrivacyPolicy, graph: &mut GraphArData) -> Result<Derived, Error> {
    let mut derived = Derived::default();

    // `k < 2` is not an anonymisation — `fossil-kanon` refuses it as the
    // identity — and it is the verifier's to reject, not this module's. Leaving
    // the corpus untouched sends it there.
    if policy.k < 2 {
        return Ok(derived);
    }

    for node in &graph.schema.nodes.clone() {
        let Some(rule) = policy.shape(&node.label) else {
            continue;
        };
        let Some(table) = graph.vertices.iter_mut().find(|v| v.label == node.label) else {
            continue;
        };
        let plan = plan_for(rule, node, table)?;
        if plan.is_empty() {
            continue;
        }
        let tokens = generalize_table(&node.label, table, &plan, policy)?;
        derived.tokens.extend(tokens);

        // The manifest declares each property's type off the GRAPH SCHEMA and
        // not off the batches (`GraphArData::manifest` → `vertex_info`), so a
        // column that just became `Utf8` in the bytes and stayed `integer` in
        // the schema is a manifest that lies about its own Parquet. A reader
        // that believes it reads a corpus it cannot open. Both halves move
        // together or neither does.
        if let Some(n) = graph
            .schema
            .nodes
            .iter_mut()
            .find(|n| n.label == node.label)
        {
            for property in &mut n.properties {
                if plan.iter().any(|c| c.name == property.name) {
                    property.datatype = Primitive::String;
                }
            }
        }
    }

    Ok(derived)
}

/// Write what was derived onto the bound the verifier sealed.
///
/// Called **after** [`crate::privacy::verify`] and separately from it, so that
/// the value the manifest records is one the verifier measured and the note
/// beside it is one this module wrote, with no call in which either could have
/// supplied the other's half.
pub fn record(derived: &Derived, privacy: &mut Privacy) {
    if let Privacy::KAnonymity(bound) = privacy {
        bound.generalization = derived.render();
    }
}

/// One column to generalise: where it is, what it is called, and how.
struct Planned {
    name: String,
    index: usize,
    hierarchy: Hierarchy,
}

/// Resolve the shape's declared hierarchies against the columns actually
/// published, and refuse a shape that covers only some of them.
///
/// The published columns come off the Arrow schema rather than the graph schema
/// for the reason `privacy.rs` gives about `published_columns`: the bytes are
/// what a recipient gets.
fn plan_for(
    rule: &fossil_policy::ShapeRule,
    node: &NodeType,
    table: &VertexTable,
) -> Result<Vec<Planned>, Error> {
    let Some(first) = table.batches.first() else {
        return Ok(Vec::new());
    };
    let iri_of = |name: &str| {
        node.properties
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.iri.clone())
    };

    let mut planned = Vec::new();
    let mut quasi_identifiers = 0usize;
    let mut missing: Vec<String> = Vec::new();
    for (index, field) in first.schema().fields().iter().enumerate() {
        let name = field.name().clone();
        let iri = iri_of(&name);
        let iri = iri.as_deref();
        // The same tuple `privacy::resolve_quasi_identifiers` will measure over:
        // the policy's set where it declared one, its classification otherwise.
        let is_qi = if rule.quasi_identifiers.is_empty() {
            rule.classify(iri, &name) == fossil_policy::Classification::QuasiIdentifier
        } else {
            rule.quasi_identifiers
                .iter()
                .any(|key| iri == Some(key.as_str()) || key == &name)
        };
        if !is_qi {
            continue;
        }
        quasi_identifiers += 1;

        let Some(declared) = rule.hierarchy(iri, &name) else {
            missing.push(name);
            continue;
        };
        let hierarchy: Hierarchy =
            serde_json::from_value(declared.clone()).map_err(|e| Error::Malformed {
                shape: node.label.clone(),
                column: name.clone(),
                reason: e.to_string(),
            })?;
        if let Hierarchy::Numeric(n) = &hierarchy
            && n.presentation == NumericPresentation::ObservedRange
        {
            return Err(Error::ObservedRangeRefused {
                shape: node.label.clone(),
                column: name,
            });
        }
        planned.push(Planned {
            name,
            index,
            hierarchy,
        });
    }

    // All of them or none of them. A shape with no hierarchies at all is the
    // pre-derivation corpus and is left alone; one with some is a policy whose
    // author believes they generalised this type.
    if !planned.is_empty() && !missing.is_empty() {
        return Err(Error::PartiallyCovered {
            shape: node.label.clone(),
            generalised: planned.len(),
            total: quasi_identifiers,
            missing: missing.join(", "),
        });
    }
    Ok(planned)
}

/// Concatenate the release, generalise it as one table, and slice it back into
/// the tiles it arrived as.
fn generalize_table(
    shape: &str,
    table: &mut VertexTable,
    plan: &[Planned],
    policy: &PrivacyPolicy,
) -> Result<Vec<String>, Error> {
    let lengths: Vec<usize> = table.batches.iter().map(RecordBatch::num_rows).collect();
    let rows: usize = lengths.iter().sum();
    if rows == 0 {
        return Ok(Vec::new());
    }

    // ONE table, every tile. See the module docs on the scope trap; this
    // concatenation is the whole of the mitigation and the slice below is what
    // makes it invisible to the rest of the write path.
    let mut whole: Vec<ArrayRef> = Vec::with_capacity(plan.len());
    for column in plan {
        let parts: Vec<&dyn Array> = table
            .batches
            .iter()
            .map(|b| b.column(column.index).as_ref())
            .collect();
        whole.push(concat(&parts).map_err(|e| Error::Arrow {
            shape: shape.to_string(),
            column: column.name.clone(),
            source: e,
        })?);
    }

    let qis: Vec<QuasiIdentifier> = plan
        .iter()
        .zip(&whole)
        .map(|(c, values)| QuasiIdentifier {
            name: c.name.clone(),
            values: Arc::clone(values),
            hierarchy: c.hierarchy.clone(),
        })
        .collect();

    let out = fossil_kanon::anonymize(
        &qis,
        &Config {
            k: policy.k as usize,
            nulls: nulls_for(policy.absent_quasi_identifier),
        },
    )
    .map_err(|source| Error::Kanon {
        shape: shape.to_string(),
        source,
    })?;

    // Re-apply the input null mask. `anonymize` renders a missing
    // quasi-identifier as `*`, and a `*` in the corpus is a value as far as
    // every reader downstream is concerned — including the verifier, whose
    // suppression budget counts nulls and would count none. See the module docs.
    let published: Vec<ArrayRef> = out
        .columns
        .iter()
        .zip(&whole)
        .map(|(generalised, source)| {
            let strings = generalised
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("`anonymize` publishes one StringArray per input column");
            let cells: Vec<Option<String>> = (0..rows)
                .map(|r| {
                    if source.is_null(r) || strings.is_null(r) {
                        None
                    } else {
                        Some(strings.value(r).to_string())
                    }
                })
                .collect();
            Arc::new(StringArray::from(cells)) as ArrayRef
        })
        .collect();

    // The schema, once: the generalised columns become nullable `Utf8` and
    // everything else is untouched. Nullable because a withheld row publishes
    // nothing, and because the mask above can introduce a null into a column
    // that had none.
    let old = table.batches[0].schema();
    let fields: Vec<Arc<Field>> = old
        .fields()
        .iter()
        .enumerate()
        .map(|(i, f)| {
            if plan.iter().any(|c| c.index == i) {
                Arc::new(Field::new(f.name(), DataType::Utf8, true))
            } else {
                Arc::clone(f)
            }
        })
        .collect();
    let schema = Arc::new(Schema::new(fields));

    // Back into tiles of exactly the lengths they arrived as. The corpus's
    // tiling is not this module's to change — `fossil-layout` decides it, and a
    // deriver that re-tiled would move rows between row groups behind the
    // layout pass's back.
    let mut offset = 0usize;
    let mut batches = Vec::with_capacity(table.batches.len());
    for (batch, len) in table.batches.iter().zip(&lengths) {
        let columns: Vec<ArrayRef> = (0..batch.num_columns())
            .map(|i| match plan.iter().position(|c| c.index == i) {
                Some(p) => published[p].slice(offset, *len),
                None => Arc::clone(batch.column(i)),
            })
            .collect();
        batches.push(
            RecordBatch::try_new(Arc::clone(&schema), columns).map_err(|e| Error::Arrow {
                shape: shape.to_string(),
                column: "<tile>".to_string(),
                source: e,
            })?,
        );
        offset += len;
    }
    table.batches = batches;

    Ok(plan
        .iter()
        .zip(&out.report.columns)
        .map(|(c, report)| format!("{shape}.{}@{}", c.name, levels(&report.levels)))
        .collect())
}

/// How a column's generalisation is written on the manifest.
///
/// A levelled column reports the coarsest and finest declared level any
/// published class sits at, over the levels the hierarchy declares. A numeric
/// column has no level to report — Mondrian cuts it at partition medians, and
/// `ColumnLevels` refuses to invent a lattice position for spans that nest in no
/// lattice — but [`apply`] admits only `enclosing_bucket`, so every published
/// value IS a declared bucket edge and `bucket` is the honest name for that. The
/// widths themselves are `f64`s and stay off a manifest four independent readers
/// parse: a float is the one scalar they can disagree about over the same bytes.
fn levels(levels: &ColumnLevels) -> String {
    match levels {
        ColumnLevels::Levels {
            coarsest,
            finest,
            declared,
        } => format!("{coarsest}-{finest}/{declared}"),
        ColumnLevels::Spans { .. } => "bucket".to_string(),
    }
}

/// The anonymiser's null policy for a declared reading of an absent
/// quasi-identifier.
///
/// Two of the three map straight across. [`AbsentQuasiIdentifier::Value`] has no
/// counterpart — `fossil-kanon` offers the wildcard reading and suppression, and
/// not «a null is a category» — so it takes the **conservative** one.
///
/// That direction is the one that matters. Under `Suppress` the partitioner
/// never sees a wildcard, so the strict cut test degenerates to «both sides have
/// k members» and the released rows reach k under exact tuple matching, which is
/// what `Value` measures. Deriving under `Wildcard` instead would credit a null
/// row with the size of every class it could belong to, and the `Value` verifier
/// credits it with nothing of the sort — the deriver would claim a bound the
/// check does not grant.
///
/// The cost is real and lands in the safe direction: a corpus with null
/// quasi-identifiers and a `value` reading suppresses rows the wildcard reading
/// would have released, and those rows are charged to the suppression budget. A
/// corpus with no nulls — which is the common one, and the one every column
/// without a null bit is in — sees no difference at all, because the two
/// policies coincide exactly there.
const fn nulls_for(absent: AbsentQuasiIdentifier) -> NullPolicy {
    match absent {
        AbsentQuasiIdentifier::Wildcard => NullPolicy::Wildcard,
        AbsentQuasiIdentifier::Value | AbsentQuasiIdentifier::Suppress => NullPolicy::Suppress,
    }
}
