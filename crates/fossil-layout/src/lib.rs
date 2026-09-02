//! `fossil-layout`: the layout post-pass. **The crate IS the pass** — it was
//! `fossil-runtime` and it is not a runtime.
//!
//! [`layout`] holds no database connection: it reads and writes Parquet through
//! `arrow-rs`/`parquet-rs`, and Louvain and Morton were always Rust. `DuckDB` is
//! a **dev**-dependency, brought by the tests to read back what the library
//! wrote — `docs/design/one-engine.mdx` has the argument. The crate does still
//! link `DataFusion`, transitively through `fossil-df`, for the edge below.
//!
//! **This crate compiles for `wasm32`**, which is what the `DuckDB` demotion
//! bought.
//!
//! # The one edge to `fossil-df`, and it stays
//!
//! `layout.rs` takes exactly one thing from the language side:
//! `fossil_df::files`, the single Arrow→Parquet writer — `TileWriter` here,
//! `batches_to_parquet` on the staging side — so that the row-group-per-tile
//! property a reader indexes on is stated in one place. That is a corpus crate
//! depending on an engine crate, and both obvious
//! repairs cost more than the edge does: moving the encoder to `fossil-sinks`
//! puts `arrow` + `parquet` into `fossil-graph-wasm`'s browser bundle, which has
//! neither, and a leaf crate of its own buys one shared function for a whole new
//! crate. **The decision is to leave the edge and pay for it when there is a
//! second reason** — a second consumer of the encoder that is not on the engine
//! side.

pub mod io;
pub mod layout;
