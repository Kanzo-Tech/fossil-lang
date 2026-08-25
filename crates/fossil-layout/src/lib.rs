//! `fossil-layout`: the layout post-pass, and it links no engine.
//!
//! It was `fossil-runtime`, and both words of "native execution runtime" had
//! stopped being true: `graph_exec.rs` left in `d6f957f` (it is
//! `fossil_mcp::executor` now), `materialize.rs` was deleted in `1e11a91`, and
//! what is left is one module. **The crate IS the layout pass**, so it is named
//! after it and it sits with the corpus rather than with the engine.
//!
//! **This crate compiles for `wasm32`.** It carried a `compile_error!` naming
//! bundled `DuckDB` as the reason it could not, and `docs/design/one-engine.mdx`
//! opens on what that cost: *«the sentence this project is built on — fossil is
//! a compiler consumed as a WASM library — is true of `fossil-wasm` and false of
//! the layout pass and the engine. One dependency is what makes it half true.»*
//! The dependency is gone from here. `fossil-engine` still has one.
//!
//! What is left is [`layout`], which holds no connection: it reads and writes
//! Parquet through `arrow-rs`/`parquet-rs` and its algorithm — Louvain, Morton —
//! was always Rust.
//!
//! # The one edge to `fossil-df`, and it stays
//!
//! `layout.rs` takes exactly one thing from the language side:
//! `fossil_df::files::batches_to_parquet`, the single Arrow→Parquet encoder.
//! That is a corpus crate depending on an engine crate, and the obvious repairs
//! both cost more than the edge does. Moving the encoder to `fossil-sinks` puts
//! `arrow` + `parquet` into `fossil-graph-wasm`'s browser bundle, which today
//! has neither; giving it a leaf crate of its own buys one shared function for
//! a twenty-seventh crate. **The decision is to leave the edge and pay for it
//! when there is a second reason** — a second consumer of the encoder that is
//! not on the engine side. Until then, do not re-derive this: the tiles are
//! emitted through the same writer as the sink so that the
//! row-group-per-tile property a reader indexes on is stated in one place.
//!
//! `DuckDB` is a **dev**-dependency, and the demotion is the point rather than a
//! technicality. Three test files bring an engine to read back what the library
//! wrote, which `one-engine.mdx` calls *«a test convenience, not a dependency of
//! the language»* and which `tests/layout_renumber.rs` argues for in its own
//! header: *«a corpus only one writer can read is a corpus»*. The library links
//! nothing; the tests check somebody else can read it.
//!
//! # What left, and it had no callers
//!
//! `execute(sql)` — *«Callers feed a batch of SQL statements straight to
//! `execute`»* — had **no production caller anywhere in the tree**. Its callers
//! were `tests/assertion_negative.rs` and this module's own `#[cfg(test)]`
//! block. The pipeline it documented, `CREATE VIEW … read_csv_auto(…)` followed
//! by `COPY (…) TO … (FORMAT PARQUET)`, is emitted by nothing: `grep` for
//! `CREATE VIEW` across every `src/` finds this doc comment and
//! `fossil_graph::executor`'s verb SQL, which is the corpus READ side. The compile
//! path went to `fossil-df` (DataFusion/Arrow) and this stayed behind.
//!
//! `apply_memory_budget(conn, bytes)` had **no caller at all**, not even a test
//! outside its own. What it capped was the `DuckDB` layout pass, and
//! `fossil_engine::enrich_written_layout` records in its own comment that the
//! pass no longer opens a connection. Two comments named the function; neither
//! called it, and both were honest about that (*«used to»*, *«when it does»*).
//!
//! And the three that were merely dead: `fossil-sinks` (named by no `.rs` in
//! this crate at all), `fossil-resolver` (which went with `materialize.rs` —
//! and which carries its OWN wasm32 `compile_error!`, so the unused edge kept
//! this crate off wasm32 even after `DuckDB` left), and `serde_json`, which
//! only ever served one test and is a dev-dependency now.
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

// `pub mod graph_exec;` lived here — `ConnectionExecutor`, then called
// `DuckRuntime` after this crate, which is the native side of the
// corpus verbs. It reads a corpus; this crate WRITES one, and the two only
// shared a `Connection`. It was also the single `fossil-layout -> fossil-graph`
// edge, and that edge ran backwards: the write path of the language depending
// on the read path of the format. It is `fossil-mcp`'s now, which was its only
// caller and already depended on `fossil-graph`.
pub mod layout;
