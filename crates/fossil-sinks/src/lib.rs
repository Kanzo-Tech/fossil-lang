//! What a sink is handed, and the sinks: one module per output format, each
//! behind the cargo feature of its name.
//!
//! The engine (`fossil-exec`) runs a program into a [`Graph`] — the program's
//! graph type and the relations of its vertices and edges as Arrow — and a sink
//! writes it in one format. A host links the sinks it builds and no others, so a
//! module that only writes `GraphAr` does not carry the code of another format.
//!
//! - [`graphar`] (feature `graphar`): an Apache `GraphAr` graph, the chunks
//!   through `graphar-parquet`, `mapping.ttl` after them and `fossil.graph.yml`
//!   last.

// The writer is single-threaded by construction, as the engine is: the browser
// has one thread, and `DataFusion`'s futures are not `Send`.
#![allow(clippy::future_not_send)]
// `result_large_err`: a write refuses with `fossil_graph_schema::Failure`, for
// the reason `fossil-exec`'s crate root gives.
#![allow(clippy::result_large_err)]

use datafusion::arrow::record_batch::RecordBatch;
#[cfg(doc)]
use fossil_graph_schema::Cardinality;
use fossil_graph_schema::GraphSchema;
use serde::{Deserialize, Serialize};

#[cfg(feature = "graphar")]
pub mod graphar;

/// The columns of the tables the engine holds between the executor and the
/// writer. They are not the graph's: the writer numbers the rows within their
/// type, names the identity `subject` as a property, and turns the rest into
/// `GraphAr`'s columns.
///
/// The rank of a vertex in the whole graph, `u32`: the types in schema order,
/// each one a contiguous range of it.
pub const PAYLOAD_ADDRESS: &str = "dense_id";
/// The `dense_id` of the vertex a multi-valued property's value belongs to.
pub const ENDPOINT_SRC: &str = "src";
/// The BCP 47 tag of a language-tagged value, beside it.
pub const PROPERTY_LANGUAGE: &str = "lang";

/// The materialised graph for a program: the canonical [`GraphSchema`] (the
/// single source of all type/predicate/cardinality metadata) plus the relation
/// data — vertex tables and edge tables as in-memory `RecordBatch`es.
///
/// This is the universal substrate made concrete: **relations + a
/// graph-schema**. A sink writes the graph *from* this; the data
/// carriers hold no metadata of their own — it all lives in
/// [`schema`](Self::schema).
#[derive(Debug)]
pub struct Graph {
    pub schema: GraphSchema,
    pub vertices: Vec<VertexTable>,
    pub edges: Vec<EdgeTable>,
}

/// A materialised edge's rows, `ORDER BY src_dense, dst_dense`: two `u32`
/// columns, each its endpoint's global `dense_id`. The `(src_type, label,
/// dst_type)` triple identifies the edge in the schema; all other metadata is
/// in the [`GraphSchema`].
#[derive(Debug)]
pub struct EdgeTable {
    pub label: String,
    pub src_type: String,
    pub dst_type: String,
    pub batches: Vec<RecordBatch>,
    /// The sources its rows were derived from, as `fossil_exec::plan::derived_from` spells
    /// them, over every mapping that writes it.
    pub derived_from: Vec<String>,
    /// Rows of this edge's input that resolved no endpoint pair, and so are not
    /// in [`Self::batches`]. See `fossil_exec::execute_edge` for why they are
    /// discarded and [`EdgeDrops`] for where the number goes.
    pub dropped: u64,
}

/// A materialised vertex relation: the `label`-named table and its
/// `RecordBatch`es in the writer-W0b column shape (`dense_id`-prefixed). The
/// batches are also registered in the executor's `SessionContext` under
/// `label` so the edge phase joins them without re-reading Parquet. The node's
/// type/property metadata lives in the [`GraphSchema`], keyed by this `label`.
///
/// A [`Cardinality::Multi`] property is not a column of it: its values are in
/// [`Self::properties`], a table each.
#[derive(Debug)]
pub struct VertexTable {
    pub label: String,
    pub batches: Vec<RecordBatch>,
    /// The sources its rows were derived from, as `fossil_exec::plan::derived_from` spells
    /// them, over every mapping that emits the type.
    pub derived_from: Vec<String>,
    pub properties: Vec<PropertyValues>,
}

/// A multi-valued property's values, in first normal form: `src`, the
/// `dense_id` of the vertex a value belongs to, and the value under the
/// property's name — a set, `ORDER BY src, value`.
#[derive(Debug)]
pub struct PropertyValues {
    pub name: String,
    pub batches: Vec<RecordBatch>,
    /// The sources of the mappings that write the property — a subset of its
    /// vertex's [`VertexTable::derived_from`].
    pub derived_from: Vec<String>,
}

/// How many rows of one relation's input did not become an edge.
///
/// `execute_edge` resolves both endpoints against the vertex tables with an
/// inner join, so a row naming a subject no vertex carries is discarded. **That
/// is intended and it stays** — a graph cannot hold an edge to a vertex that is
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
