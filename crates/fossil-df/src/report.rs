//! What a run tells its caller: the two facts the corpus cannot hold about
//! itself.
//!
//! **The manifest is not here.** It is `<dest>/fossil.json`, written last, and
//! a host that wants to know what it just produced reads it — or opens the
//! corpus, which reads it. The report used to carry the whole manifest a second
//! time, and before that a `RunStatus` that restated seven of its nine fields
//! and once named a path the write had just deleted. Two descriptions of one
//! dataset can disagree with the bytes; one cannot.
//!
//! What remains is what the bytes cannot say:
//!
//! [`RunReport::dest`] — the caller chose it, and a corpus does not know where
//! it is.
//!
//! [`RunReport::dropped`] — how many rows of each relation's input did not
//! become an edge, because an endpoint named a subject no vertex carries. That
//! is provenance: a fact about the write, not about the bytes. See
//! [`EdgeDrops`].
//!
//! What would put the manifest back: a consumer that needs the counts without
//! reading the store — then the report gains `manifest`, of the type
//! `fossil_sinks::manifest::Manifest`, and still no second shape.

use serde::{Deserialize, Serialize};

/// Where a run wrote, and what the write discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunReport {
    /// The prefix the corpus was written under, `fossil.json` at its root.
    pub dest: String,
    /// One entry per relation, in the order `fossil.json` lists its edge
    /// tables.
    pub dropped: Vec<EdgeDrops>,
}

/// How many rows of one relation's input did not become an edge.
///
/// `execute_edge` resolves both endpoints against the vertex tables with an
/// inner join, so a row naming a subject no vertex carries is discarded. **That
/// is intended and it stays** — a corpus cannot hold an edge to a vertex that is
/// not there — and it is counted rather than silent.
///
/// # Why the count is here and not in `fossil.json`
///
/// No reader can verify it: `record_count` earns its place in the manifest by
/// being answerable to the Parquet, and a dropped count is about rows that are
/// not there and never were. And a third-party writer has no such number — it
/// performs no join — so a manifest field for it would make a conforming corpus
/// impossible to write without a fact only fossil's pipeline has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EdgeDrops {
    /// The edge table's name in `fossil.json`, `<Src>_<label>_<Dst>`.
    pub table: String,
    /// Rows of its input that resolved no endpoint pair.
    pub dropped: u64,
}
