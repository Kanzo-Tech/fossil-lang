//! `fossil-runtime`: the layout post-pass, and the `DuckDB` secret seam two
//! hosts still share.
//!
//! This crate is **NATIVE-ONLY** by design and the `compile_error!` below says
//! so at compile time rather than at run time. What makes it native is now ONE
//! thing — [`materialize`], which installs a `CREATE SECRET` on a caller's
//! `DuckDB` connection. [`layout`] holds no connection: it reads and writes
//! Parquet with `arrow-rs`.
//!
//! # What left, and it had no callers
//!
//! `execute(sql)` — *«Callers feed a batch of SQL statements straight to
//! `execute`»* — had **no production caller anywhere in the tree**. Its callers
//! were `tests/assertion_negative.rs` and this module's own `#[cfg(test)]`
//! block. The pipeline it documented, `CREATE VIEW … read_csv_auto(…)` followed
//! by `COPY (…) TO … (FORMAT PARQUET)`, is emitted by nothing: `grep` for
//! `CREATE VIEW` across every `src/` finds this doc comment and
//! `fossil_graph::exec`'s verb SQL, which is the corpus READ side. The compile
//! path went to `fossil-df` (DataFusion/Arrow) and this stayed behind.
//!
//! `apply_memory_budget(conn, bytes)` had **no caller at all**, not even a test
//! outside its own. What it capped was the `DuckDB` layout pass, and
//! `fossil_engine::enrich_written_layout` records in its own comment that the
//! pass no longer opens a connection. Two comments named the function; neither
//! called it, and both were honest about that (*«used to»*, *«when it does»*).
//!
//! `tests/assertion_negative.rs` went with them, and it is the one worth
//! stating. It transcribed
//! `CASE WHEN <guard> THEN <value> ELSE error('fossil_assertion_<name>:line=<N>') END`
//! as *«the exact shape codegen emits»* and proved `DuckDB` raises on it.
//! **Nothing emits `fossil_assertion_`** — the string appears in no `src/` file
//! in the workspace. And the coverage that replaces it already existed before
//! this deletion: `error()` has no `DataFusion` equivalent, which
//! `fossil_df::stdlib`'s `UNREACHABLE_ON_DATAFUSION` pins as the sole cause for
//! `core.require` and the four `validate.*` rows not rendering — measured by
//! planning the real template, not guessed. The named-assertion contract does
//! not exist on the engine the language runs on, and that is declared where a
//! reader will meet it.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-runtime is native-only (uses bundled DuckDB C++); use duckdb-wasm in fossil-wasm"
);

// `pub mod graph_exec;` lived here — `ConnectionExecutor`, then called
// `DuckRuntime` after this crate, which is the native side of the
// corpus verbs. It reads a corpus; this crate WRITES one, and the two only
// shared a `Connection`. It was also the single `fossil-runtime -> fossil-graph`
// edge, and that edge ran backwards: the write path of the language depending
// on the read path of the format. It is `fossil-mcp`'s now, which was its only
// caller and already depended on `fossil-graph`.
pub mod layout;
pub mod materialize;
// `pub mod udf;` lived here — eight native Rust UDF trampolines
// (`fossil_slug`, `fossil_validate_email`, `fossil_hmac`, …) registered on a
// DuckDB connection. Ruling 15 of `SURFACE-PLAN.md` deleted the `Udf` lowering
// kind: two of the eight functions left the language and the other six are SQL
// expression templates in the catalogue, so there is nothing left to register.
// The module went with them, and so did the `WasmClass` concept it was the
// whole reason for.

pub use materialize::{MaterializeError, install_secret};
