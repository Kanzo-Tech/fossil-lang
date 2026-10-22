//! The `GraphAr` sink: an Apache `GraphAr` graph, laid out from a [`Graph`]
//! and written through `graphar-parquet`, `mapping.ttl` after the chunks and
//! `fossil.graph.yml` last.
//!
//! [`Graph`]: crate::Graph

/// Every table laid out as `GraphAr`'s, held to what an RDF reader can recover.
mod layout;
/// The graph, written: `GraphAr`'s chunks, `fossil.graph.yml` last.
mod write;

/// The graph writer, and what it answers.
pub use write::{WriteError, Written, write};
