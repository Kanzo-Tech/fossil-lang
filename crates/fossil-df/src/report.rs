//! What a run tells its caller: the two facts the graph cannot hold about
//! itself.
//!
//! **The graph's description is not here.** It is `<location>fossil.graph.yml`,
//! written last, and a host that wants to know what it just produced reads it —
//! or attaches the graph, which reads it. A report that restated it could name a
//! path the write had just deleted: two descriptions of one dataset can
//! disagree with the bytes; one cannot.
//!
//! What remains is what the bytes cannot say:
//!
//! [`RunReport::location`] — the caller chose it, and a graph does not know where
//! it is.
//!
//! [`RunReport::dropped`] — how many rows of each relation's input did not
//! become an edge, because an endpoint named a subject no vertex carries. That
//! is provenance: a fact about the write, not about the bytes. See
//! [`EdgeDrops`].
//!
//! What would put the description back: a consumer that needs the counts without
//! reading the store — then the report gains `graph`, of the type
//! `graphar_info::GraphInfo`, and still no second shape.

use serde::{Deserialize, Serialize};

/// Where a run wrote, and what the write discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunReport {
    /// The prefix the graph was written under, `fossil.graph.yml` at its root.
    pub location: String,
    /// One entry per relation, in the order `fossil.graph.yml` lists its edge
    /// types.
    pub dropped: Vec<EdgeDrops>,
}

/// How many rows of one relation's input did not become an edge.
///
/// `execute_edge` resolves both endpoints against the vertex tables with an
/// inner join, so a row naming a subject no vertex carries is discarded. **That
/// is intended and it stays** — a corpus cannot hold an edge to a vertex that is
/// not there — and it is counted rather than silent.
///
/// # Why the count is here and not in the graph
///
/// No reader can verify it: a vertex or edge count earns its place in the graph
/// by being answerable to the Parquet, and a dropped count is about rows that
/// are not there and never were. And a third-party writer has no such number — it
/// performs no join — so a field for it would make a conforming graph impossible
/// to write without a fact only fossil's pipeline has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EdgeDrops {
    /// The edge type's name in `fossil.graph.yml`, `<Src>_<label>_<Dst>`.
    pub table: String,
    /// Rows of its input that resolved no endpoint pair.
    pub dropped: u64,
}
