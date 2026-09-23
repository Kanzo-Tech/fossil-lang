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
//! Not the database — introspection and credentials are `fossil-introspect`'s,
//! and `fossil-layout` links no `DuckDB`. The reason was written as one of those
//! twice and neither survived; do not write it a third time.
//!
//! What is left is [`host::run`], and it is the FILESYSTEM. It writes a
//! `GraphAr` tree to a local directory — the layout pass through
//! `fossil_layout::io::LocalFs`, then `GraphArData::write_manifests` over it —
//! and a browser has no concept of one. `fossil_df::materialise`, which the run
//! calls first, is `cfg(not(wasm32))` for a **different** reason worth keeping
//! straight: it blocks on a private `tokio` runtime. It used to be gated for the
//! filesystem too, because it ended by writing the manifests; it writes nothing
//! now. That does not contradict «fossil is a compiler consumed as a WASM
//! library»: writing files to a disk is what a native host DOES, and the
//! browser's host writes bytes back over its own seam.
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
     directory, and calls `fossil_df::materialise`, which blocks on a private \
     tokio runtime and is itself wasm-gated. Do not add it to the WASM CI gate."
);

mod documents;
pub mod host;
mod system;

pub use host::{CheckOutcome, check, providers, refs, run};
pub use system::{host_system, open_db};
