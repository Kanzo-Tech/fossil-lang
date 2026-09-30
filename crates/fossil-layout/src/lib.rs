//! `fossil-layout`: the layout pass. **The crate IS the pass.**
//!
//! [`layout`] takes the executor's Arrow rows and answers where every vertex
//! goes and what its global `dense_id` is. It writes nothing — `fossil-df`
//! writes the corpus from what it answers — and it links no engine: Louvain and
//! the curve are Rust, and `DuckDB` is a **dev**-dependency the tests bring to
//! check the curve against `ST_Hilbert`. `docs/design/one-engine.mdx` has the
//! argument.
//!
//! **This crate compiles for `wasm32`**, and it reaches `fossil-df` over no
//! edge, so `fossil-df` depends on it.

pub mod layout;
