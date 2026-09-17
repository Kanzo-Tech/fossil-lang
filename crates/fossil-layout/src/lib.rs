//! `fossil-layout`: the layout post-pass. **The crate IS the pass** — it was
//! `fossil-runtime` and it is not a runtime.
//!
//! [`layout`] holds no database connection: it takes Arrow in and writes Parquet
//! through `arrow-rs`/`parquet-rs`, and Louvain and Morton were always Rust.
//! `DuckDB` is a **dev**-dependency, brought by the tests to read back what the
//! library wrote — `docs/design/one-engine.mdx` has the argument. It links no
//! engine at all now: `DataFusion` reached this crate transitively, through the
//! edge below, and that edge is gone.
//!
//! **This crate compiles for `wasm32`**, which is what the `DuckDB` demotion
//! bought.
//!
//! # The edge to `fossil-df`, and what became of it
//!
//! `layout.rs` took exactly one thing from the language side —
//! `fossil_df::files::TileWriter` — and paid `datafusion`, `fossil-hir`,
//! `fossil-mir`, `fossil-descriptors-output`, `fossil-base` and `salsa` for it,
//! because a struct that `fossil-df` never called sat inside `fossil-df`. The
//! writer is [`fossil_tile_writer::TileWriter`] now: a leaf over `arrow` and
//! `parquet` and nothing else, still the **only** writer of the corpus's
//! payload, so the row-group-per-tile property a reader indexes on is still
//! stated in one place.
//!
//! This page said the repair cost more than the edge and named the second
//! consumer as the thing that would change its mind. What changed it instead was
//! the first consumer's closure: `fossil-sinks` is still the wrong home (it
//! takes `arrow-schema` alone and byte-writes nothing), but the leaf crate buys
//! a compiler substrate out of a batch pass, which is not what "one shared
//! function" was weighed against.
//!
//! `fossil-df` survives as a **dev**-dependency: `examples/compaction_pass.rs`
//! measures the tiling against `batches_to_parquet`, the baseline encoder, and a
//! bench's dependency is not the library's.

pub mod io;
pub mod layout;
