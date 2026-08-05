//! `fossil-runtime`: native `DuckDB` execution for the `fossil compile`/`fossil run` pipeline.
//!
//! This crate is **NATIVE-ONLY** by design — `DuckDB`-WASM lives in `fossil-wasm`
//! (Phase 7 PLAY-02 wires the playground's lazy-load). See ADR-0002 and the
//! "Project Layout" section of `CLAUDE.md`. The `compile_error!` cfg-tripwire
//! below catches accidental inclusion of `fossil-runtime` in the WASM CI gate
//! at compile time rather than runtime.
//!
//! Phase 1 callers feed the output of [`fossil_codegen::codegen_sql`] (the
//! `sql` field of [`fossil_codegen::SqlPlan`]) directly to [`execute`]. The
//! crate runs OUTSIDE the Salsa query graph (per `architecture.md` "Runtime
//! boundary"): Salsa terminates when the SQL plan is emitted; the runtime
//! takes over from there.
//!
//! Phase 5 STDL-05 will register Rust UDFs on the connection before
//! `execute_batch` is called. Phase 6 CLI-01..03 wraps `duckdb::Error` in
//! `miette::Diagnostic` for CLI display. Phase 1 deliberately keeps the API
//! to a single function returning the raw `duckdb::Error` — see RESEARCH.md
//! §"Phase 1 Recommended Commit Strategy" commit #7.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-runtime is native-only (uses bundled DuckDB C++); use duckdb-wasm in fossil-wasm"
);

use duckdb::Connection;

pub mod graph_exec;
pub mod layout;
pub mod materialize;
pub mod udf;

pub use graph_exec::DuckRuntime;
pub use materialize::{MaterializeError, install_secret};

/// Execute a batch of SQL statements (semicolon-delimited) on a fresh
/// in-memory `DuckDB` connection.
///
/// The expected input is the `sql` field of a
/// [`fossil_codegen::SqlPlan`](https://docs.rs/fossil-codegen) — a
/// `CREATE VIEW … read_csv_auto(…)` followed by a
/// `COPY (…) TO 'output.parquet' (FORMAT PARQUET)`. Side effects from the
/// `COPY` statement write to the process's current working directory unless
/// the SQL embeds an absolute path.
///
/// Before the batch runs, every `native_udf_only` stdlib function is
/// registered on the connection via [`udf::register_stdlib_udfs`] (STDL-05),
/// so a generated `fossil_slug(x)` / `fossil_validate_email(x)` /
/// `fossil_hmac(x, k)` call resolves natively. These UDFs are unavailable in
/// `DuckDB`-WASM — the playground reads the classification manifest and
/// disables them in-browser (STDL-07).
///
/// # Errors
///
/// Returns the underlying [`duckdb::Error`] if the in-memory connection
/// cannot be opened, a UDF fails to register, or any statement in the batch
/// fails to execute.
pub fn execute(sql: &str) -> Result<(), duckdb::Error> {
    let conn = Connection::open_in_memory()?;
    udf::register_stdlib_udfs(&conn)?;
    conn.execute_batch(sql)?;
    Ok(())
}

/// Bound `conn` to the run's declared memory budget, in bytes.
///
/// The same number `fossil_df::run_to_dir` builds its `FairSpillPool` from: a
/// run declares one budget and both engines that spend memory under it honour
/// it — the `DataFusion` write path, and the `DuckDB` layout pass that rewrites
/// the vertex Parquet afterwards. `DuckDB`'s own default targets ~80% of
/// physical RAM, i.e. a whole machine per process, which is the wrong default
/// for a host running several runs at once.
///
/// `temp_directory` is set with the limit, not instead of it: an in-memory
/// database told it may not allocate and given nowhere to spill errors out
/// where it could have gone slower. Same destination the `DataFusion` disk
/// manager picks by default, so both engines overflow to the same place.
///
/// Call right after opening a connection used for a run; a run without a
/// declared budget does not call it at all (unbounded, `DuckDB`'s default).
///
/// # Errors
///
/// Returns the underlying [`duckdb::Error`] if a `SET` statement fails.
pub fn apply_memory_budget(conn: &Connection, bytes: u64) -> Result<(), duckdb::Error> {
    // `B` (not GiB): the byte count is the budget's canonical form, so DuckDB
    // and the DataFusion pool get literally the same number rather than two
    // roundings of it. The temp dir is machine-supplied — escape it as a SQL
    // literal rather than trusting it has no quote.
    let tmp = std::env::temp_dir();
    conn.execute_batch(&format!(
        "SET memory_limit='{bytes}B'; SET temp_directory='{}';",
        tmp.to_string_lossy().replace('\'', "''"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// Materialise a per-process scratch directory containing a 5-row
    /// `users.csv`. Memoised via `OnceLock` so multiple tests share the
    /// fixture without racing on `create_dir_all`.
    fn temp_workdir() -> &'static PathBuf {
        static D: OnceLock<PathBuf> = OnceLock::new();
        D.get_or_init(|| {
            let p = std::env::temp_dir().join("fossil-runtime-test");
            std::fs::create_dir_all(&p).expect("create scratch dir");
            std::fs::write(
                p.join("users.csv"),
                "id,name\n1,Alice\n2,Bob\n3,Carol\n4,Dave\n5,Eve\n",
            )
            .expect("write users.csv fixture");
            p
        })
    }

    /// The second engine actually receives the run's number. `memory_limit` is
    /// reported back in `DuckDB`'s own formatting, so the assertion is on the
    /// setting having moved off the machine-sized default, plus a temp directory
    /// to spill into — a limit without one errors where it could go slower.
    #[test]
    fn a_declared_budget_reaches_duckdb() {
        let conn = Connection::open_in_memory().expect("open in-memory duckdb");
        apply_memory_budget(&conn, 1024 * 1024 * 1024).expect("apply the budget");

        let setting = |name: &str| -> String {
            conn.query_row(&format!("SELECT current_setting('{name}')"), [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        assert_eq!(setting("memory_limit"), "1.0 GiB");
        assert!(
            !setting("temp_directory").is_empty(),
            "a memory limit with nowhere to spill is an error, not a slower run",
        );
    }

    #[test]
    fn execute_creates_output_parquet_with_5_triple_rows() {
        let dir = temp_workdir();
        let csv_path = dir.join("users.csv");
        let out_path = dir.join("output.parquet");
        // Best-effort cleanup of any prior run's artefact so existence
        // assertions below mean "this run wrote it".
        let _ = std::fs::remove_file(&out_path);

        // Mirror the snapshot SQL shape from
        // crates/fossil-codegen/tests/snapshots/compile_hello__hello_sql.snap
        // — CREATE VIEW + COPY (…) TO '…' (FORMAT PARQUET) — but with absolute
        // paths so the test does not depend on the working directory.
        let sql = format!(
            "CREATE VIEW users AS SELECT * FROM read_csv_auto('{csv}', sample_size=-1);\n\
             COPY (\n\
                 SELECT 'https://example.org/user/' || users.id AS subject,\n\
                        'https://example.org/name' AS predicate,\n\
                        users.name AS object\n\
                 FROM users\n\
             ) TO '{out}' (FORMAT PARQUET);",
            csv = csv_path.display(),
            out = out_path.display(),
        );

        execute(&sql).expect("SQL execution should succeed");

        assert!(out_path.exists(), "output.parquet should exist");
        let metadata = std::fs::metadata(&out_path).expect("stat output.parquet");
        assert!(metadata.len() > 0, "output.parquet should be non-empty");

        // Read back through DuckDB to confirm the Parquet file is valid and
        // matches the 5-row hello.fossil expectation (RESEARCH.md Example 3).
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT COUNT(*) FROM read_parquet('{out}')",
                out = out_path.display(),
            ))
            .expect("prepare count query");
        let count: i64 = stmt
            .query_row([], |row| row.get(0))
            .expect("execute count query");
        assert_eq!(count, 5, "should have 5 triples");
    }
}
