//! **`fossil-cli` is fossil's native host, and the `fossil` binary over it.**
//!
//! The library half was `fossil-engine` until phase E and is [`host`] now. It
//! had one consumer, this crate, while the other two hosts — `fossil-lsp` and
//! `fossil-wasm` — each keep theirs inside themselves. Three hosts, three
//! crates.
//!
//! The split that remains is the one that earns itself: [`host`] returns
//! structured data and renders nothing, and `src/main.rs` parses args and does
//! the rendering. That is not a crate boundary, it is a module boundary, and it
//! is the boundary it always was.
//!
//! # Why this crate is native and cannot be otherwise
//!
//! Not the database. That reason was written down twice — «depends on
//! fossil-layout which uses bundled `DuckDB`», then «its own bundled `DuckDB`, for
//! `pre_introspect`» — and neither survived: introspection and credentials are
//! `fossil-introspect`'s.
//!
//! What is left is [`host::run`], and it is the FILESYSTEM. `fossil_df::run_to_dir`
//! is itself `cfg(not(wasm32))` because it writes a `GraphAr` tree to a local
//! directory, which a browser has no concept of — measured by building for the
//! target, and it is the only remaining error. That does not contradict «fossil
//! is a compiler consumed as a WASM library»: writing files to a disk is what a
//! native host DOES, and the browser's host writes bytes back over its own seam.
//!
//! So [`host::check`] is wasm-capable and [`host::run`] is not, in one crate.
//! Splitting them is a decision and not a cleanup; `/docs/design/one-engine`
//! records it.

#![allow(rustdoc::private_intra_doc_links)] // `host::enrich_written_layout` is
// named from `host`'s own header because it is where the coupling lives, and a
// reader who follows the pointer should land on the code rather than on a
// paraphrase of it.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-cli is native-only: `run` writes a GraphAr tree to a local \
     directory through `fossil_df::run_to_dir`, which is itself wasm-gated. \
     Do not add it to the WASM CI gate."
);

mod documents;
pub mod host;
mod system;

pub use host::{CheckOutcome, check, providers, refs, run};
pub use system::{host_system, open_db};
