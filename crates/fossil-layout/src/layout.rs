//! Graph layout precompute — pure, host-agnostic algorithms, and the pass that
//! applies them to a written corpus.
//!
//! The writer emits `x`/`y`/`cluster_id` as placeholders (`0`); this fills them,
//! so a reader addressing the corpus gets meaningful positions and the Parquet
//! can be morton-sorted for predicate pushdown.
//!
//! The pure half, over a `dense_id` edge list:
//!
//! - [`community_hierarchy`] — modularity communities, and the whole hierarchy
//!   of them, which is what [`enrich_layout`] partitions by. This is the pyramid
//!   the level-of-detail plan is built from.
//! - [`weakly_connected_components`] — reachability (union-find). It is a real
//!   graph property and stays, but it is **no longer what the layout uses**: on
//!   a connected graph it answers "one component", and measured on the million
//!   corpus that put 994,786 of a million vertices in a single cluster. It now
//!   earns its place as the contrast the hierarchy is tested against.
//! - [`cluster_layout`] — a deterministic community-grouped placement: clusters
//!   on a grid, nodes phyllotaxis-packed within their cell. Same-cluster nodes
//!   land near each other. `ForceAtlas2` refinement is a later slice; this gives
//!   the viewport real, stable coordinates without an iterative force sim.
//!
//! Each is pure (no I/O, no RNG, no engine) so they unit-test in isolation and
//! [`enrich_layout`] can wire them with confidence.
//!
//! Each of those now has a file, and so does the pass that wires them. The cut
//! is the one this comment already described — the pure half against the half
//! that touches Parquet — made a module boundary rather than a paragraph:
//!
//! - [`community`] — the partition. Louvain, its quotient graph, and the
//!   reachability it is contrasted against.
//! - [`place`] — where a vertex goes on the plane, given its community.
//! - [`morton`] — Z-order codes and the quantisation into one.
//! - [`pass`] — [`enrich_layout`], which reads and writes Parquet through
//!   `parquet-rs` and `arrow-rs` and through the one encoder the writer itself
//!   uses. There is no database here: see `docs/design/one-engine.mdx`.
//!
//! **The public API is unchanged**: every name below was `layout::<name>` before
//! the split and still is. The file was 3,540 lines holding four subjects, and
//! the thing that forced the cut is that the dendrogram has no API — a caller
//! that wants to cut it at chosen sizes, which is what `/docs/design/holons`
//! asks for, had nowhere to reach.

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
pub mod morton;
pub mod pass;
pub mod place;

pub use community::{community_hierarchy, weakly_connected_components};
pub use pass::{
    AdjacencyTarget, Endpoint, LayoutError, VertexLayoutTarget, enrich_layout, enrich_layout_with,
    enrich_layout_within, estimated_peak_bytes,
};
pub use place::cluster_layout;
