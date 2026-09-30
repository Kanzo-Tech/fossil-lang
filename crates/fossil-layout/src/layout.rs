//! Graph layout — pure, host-agnostic algorithms, and the pass that wires
//! them over the whole graph.
//!
//! - [`community`] — the partition. Louvain and its quotient graph.
//! - [`place`] — where a vertex goes on the plane, given its community.
//! - [`hilbert`] — Hilbert codes, quantised as `DuckDB`'s `ST_Hilbert` does.
//! - [`pass`] — [`layout()`], which takes the executor's `RecordBatch`es and
//!   answers a [`Layout`]: a global `dense_id`, a position and a community for
//!   every vertex.
//!
//! Each of the first three is pure (no I/O, no RNG, no engine), so they
//! unit-test in isolation.

// This is deliberate numeric code: dense ids / cluster counts cast to/from `f32`
// coordinates and `f64` grid maths, and tight index loops over `dense_id` arrays.
// The pedantic cast lints + the nursery loop/option rewrites read worse here than
// the explicit arithmetic, so they are declined for this algorithmic core.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::needless_range_loop,
    clippy::option_if_let_else
)]

pub mod community;
pub mod hilbert;
pub mod pass;
pub mod place;

pub use community::community_hierarchy;
pub use pass::{Layout, LayoutError, Relation, VertexType, estimated_peak_bytes, layout};
pub use place::cluster_layout;
