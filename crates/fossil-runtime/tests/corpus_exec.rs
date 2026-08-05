//! SC#1 native-execution tier + cross-engine baseline producer.
//!
//! NATIVE-ONLY (`fossil-runtime` carries a `wasm32` `compile_error!` tripwire).
//! This is tier 2 of the two-tier parity strategy (ADR-0014):
//!
//! 1. **Snapshot tier** — `fossil-codegen/tests/corpus.rs` locks the 30-mapping
//!    corpus' generated SQL text.
//! 2. **Native-execution tier (THIS FILE)** — the executable subset's SQL runs
//!    on native `duckdb` 1.10502 against on-disk fixture CSVs; the result set is
//!    asserted byte-for-byte AND a `native_baseline.json` digest is written for
//!    the WASM tier to reproduce.
//! 3. **WASM-execution tier** — `tests/wasm_parity/run-parity.mjs` reads the
//!    baseline, re-runs each SQL on `@duckdb/duckdb-wasm` 1.33.x, and diffs the
//!    reproduced digests (the documented MANUAL phase-close gate).
//!
//! # The cross-engine result-set serialization (reproduced byte-for-byte by JS)
//!
//! For each executable entry the SQL ends with a deterministic `ORDER BY` so the
//! row order is engine-stable. The result set is serialized as:
//!
//! - each cell rendered to its UTF-8 text form (NULL → the empty string),
//! - the cells of one row joined with `|` (U+007C),
//! - the rows joined with `\n` (U+000A),
//!
//! then hashed with SHA-256. `result_sha256` is the hex digest of that string;
//! `sql_sha256` is the SHA-256 of the exact SQL text fed to the engine;
//! `row_count` is the number of result rows. `run-parity.mjs` recomputes the
//! identical digests using node's `crypto.createHash('sha256')`.
//!
//! The cell rendering uses `DuckDB`'s own `CAST(... AS VARCHAR)` (applied in the
//! SELECT list of every executable entry) so both engines render numbers /
//! strings identically and the Rust side only ever reads VARCHAR columns.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use duckdb::Connection;
use sha2::{Digest, Sha256};

/// One executable corpus entry: a name, the SQL (reads on-disk fixtures, ends in
/// a deterministic `ORDER BY`, selects only `VARCHAR`-cast columns), and the
/// fully-expected ordered result set (the byte-for-byte assertion).
struct ExecEntry {
    name: &'static str,
    sql: &'static str,
    expected_rows: &'static [&'static [&'static str]],
}

/// The executable subset of the 30-mapping corpus. These mirror the
/// codegen-snapshot corpus shapes (`fossil-codegen/tests/corpus.rs`) but in a
/// queryable `SELECT` form (the snapshot tier locks the `COPY` form; executing a
/// `SELECT` is simpler to assert and is what DuckDB-WASM runs too). Every entry
/// reads a fixture CSV under `examples/` and ends in `ORDER BY` for a stable
/// row order. All projected columns are `CAST(... AS VARCHAR)` so the result
/// serialization is engine-portable.
const EXEC_CORPUS: &[ExecEntry] = &[
    ExecEntry {
        name: "project_two_cols",
        sql: "SELECT CAST(id AS VARCHAR) AS id, CAST(name AS VARCHAR) AS name \
              FROM read_csv_auto('examples/users.csv', sample_size=-1) ORDER BY id",
        expected_rows: &[
            &["1", "Alice"],
            &["2", "Bob"],
            &["3", "Carol"],
            &["4", "Dave"],
            &["5", "Eve"],
        ],
    },
    ExecEntry {
        name: "extend_concat",
        sql: "SELECT CAST('https://example.org/' || id AS VARCHAR) AS iri \
              FROM read_csv_auto('examples/users.csv', sample_size=-1) ORDER BY id",
        expected_rows: &[
            &["https://example.org/1"],
            &["https://example.org/2"],
            &["https://example.org/3"],
            &["https://example.org/4"],
            &["https://example.org/5"],
        ],
    },
    ExecEntry {
        name: "rename_col",
        sql: "SELECT CAST(name AS VARCHAR) AS label \
              FROM read_csv_auto('examples/users.csv', sample_size=-1) ORDER BY id",
        expected_rows: &[&["Alice"], &["Bob"], &["Carol"], &["Dave"], &["Eve"]],
    },
    ExecEntry {
        name: "distinct_all",
        sql: "SELECT DISTINCT CAST(id AS VARCHAR) AS id, CAST(name AS VARCHAR) AS name \
              FROM read_csv_auto('examples/users.csv', sample_size=-1) ORDER BY id, name",
        expected_rows: &[
            &["1", "Alice"],
            &["2", "Bob"],
            &["3", "Carol"],
            &["4", "Dave"],
            &["5", "Eve"],
        ],
    },
    ExecEntry {
        name: "distinct_by",
        sql: "SELECT CAST(id AS VARCHAR) AS id, CAST(name AS VARCHAR) AS name \
              FROM (SELECT DISTINCT ON (id) id, name \
                    FROM read_csv_auto('examples/users.csv', sample_size=-1)) AS d \
              ORDER BY id",
        expected_rows: &[
            &["1", "Alice"],
            &["2", "Bob"],
            &["3", "Carol"],
            &["4", "Dave"],
            &["5", "Eve"],
        ],
    },
    ExecEntry {
        name: "union_two_sources",
        sql: "SELECT CAST(id AS VARCHAR) AS id, CAST(name AS VARCHAR) AS name FROM ( \
                SELECT id, name FROM read_csv_auto('examples/a.csv', sample_size=-1) \
                UNION \
                SELECT id, name FROM read_csv_auto('examples/b.csv', sample_size=-1) \
              ) AS u ORDER BY id",
        expected_rows: &[
            &["1", "Alice"],
            &["2", "Bob"],
            &["3", "Carol"],
            &["4", "Dave"],
        ],
    },
    ExecEntry {
        name: "r7_const_fold_extend",
        // R7 folds `'https://example.org/' || 'user'` to the constant
        // `'https://example.org/user'` — the executed SQL emits the folded IRI.
        sql: "SELECT CAST('https://example.org/user' AS VARCHAR) AS subject \
              FROM read_csv_auto('examples/users.csv', sample_size=-1) ORDER BY id",
        expected_rows: &[
            &["https://example.org/user"],
            &["https://example.org/user"],
            &["https://example.org/user"],
            &["https://example.org/user"],
            &["https://example.org/user"],
        ],
    },
    ExecEntry {
        name: "r8_drop_true_filter",
        // R8 drops the statically-true filter — the executed SQL is the
        // passthrough projection (no WHERE).
        sql: "SELECT CAST(id AS VARCHAR) AS id, CAST(name AS VARCHAR) AS name \
              FROM read_csv_auto('examples/users.csv', sample_size=-1) ORDER BY id",
        expected_rows: &[
            &["1", "Alice"],
            &["2", "Bob"],
            &["3", "Carol"],
            &["4", "Dave"],
            &["5", "Eve"],
        ],
    },
    ExecEntry {
        name: "group_by_count",
        sql: "SELECT CAST(user_id AS VARCHAR) AS user_id, CAST(COUNT(id) AS VARCHAR) AS n \
              FROM read_csv_auto('examples/orders.csv', sample_size=-1) \
              GROUP BY user_id ORDER BY user_id",
        expected_rows: &[&["1", "2"], &["2", "1"], &["3", "2"]],
    },
    ExecEntry {
        name: "aggregate_sum",
        sql: "SELECT CAST(user_id AS VARCHAR) AS user_id, CAST(SUM(amount) AS VARCHAR) AS total \
              FROM read_csv_auto('examples/orders.csv', sample_size=-1) \
              GROUP BY user_id ORDER BY user_id",
        expected_rows: &[&["1", "350"], &["2", "75"], &["3", "350"]],
    },
];

/// Locate the workspace root (the dir holding `examples/`) so the test runs from
/// any CWD: walk up from `CARGO_MANIFEST_DIR` until `examples/users.csv` exists.
fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("examples/users.csv").exists() {
            return dir;
        }
        assert!(
            dir.pop(),
            "could not locate workspace root (examples/users.csv)"
        );
    }
}

/// Run one entry's SQL on a fresh in-memory `DuckDB` and read the ordered result
/// set back as `Vec<Vec<String>>` (every column is VARCHAR by construction).
fn run_query(conn: &Connection, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = conn.prepare(sql).expect("prepare corpus SQL");
    let mut rows = stmt.query([]).expect("execute corpus SQL");
    let mut out = Vec::new();
    while let Some(row) = rows.next().expect("read row") {
        let mut cells = Vec::new();
        let mut i = 0;
        // Columns are all VARCHAR; a NULL surfaces as Option::None → "".
        // `row.get` returns Err once `i` is past the last column → stop.
        while let Ok(cell) = row.get::<usize, Option<String>>(i) {
            cells.push(cell.unwrap_or_default());
            i += 1;
        }
        out.push(cells);
    }
    out
}

/// The canonical, engine-portable serialization: cells `|`-joined, rows
/// `\n`-joined. (Documented in the module header; reproduced by run-parity.mjs.)
fn serialize_result(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|r| r.join("|"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

#[test]
fn corpus_exec_native_and_write_baseline() {
    let root = workspace_root();
    // Run from the workspace root so the relative `examples/*.csv` paths in the
    // SQL resolve regardless of the harness CWD.
    std::env::set_current_dir(&root).expect("chdir to workspace root");

    let conn = Connection::open_in_memory().expect("open in-memory duckdb");

    let mut baseline = Vec::new();
    for entry in EXEC_CORPUS {
        let rows = run_query(&conn, entry.sql);

        // Byte-for-byte result assertion (the SC#1 native-execution contract).
        let expected: Vec<Vec<String>> = entry
            .expected_rows
            .iter()
            .map(|r| r.iter().map(|c| (*c).to_string()).collect())
            .collect();
        assert_eq!(
            rows, expected,
            "native result mismatch for corpus entry {}",
            entry.name
        );

        let serialized = serialize_result(&rows);
        baseline.push(serde_json::json!({
            "name": entry.name,
            "sql": entry.sql,
            "sql_sha256": sha256_hex(entry.sql),
            "row_count": rows.len(),
            "result_sha256": sha256_hex(&serialized),
        }));
    }

    // Write the cross-engine baseline + the single-sourced SQL list the WASM
    // harness reads. Pretty-printed + sorted-key-stable so the checked-in
    // artifact has a stable diff.
    // `tests/wasm_parity/` — beside `run-parity.mjs`, which reads these two
    // files. It used to write into `crates/fossil-codegen/`, a crate deleted in
    // `af39ff4`: the directory was recreated on every test run and `members =
    // ["crates/*"]` then failed to load it, breaking every cargo command in the
    // workspace until someone deleted it again.
    let wasm_parity_dir = root.join("tests/wasm_parity");
    std::fs::create_dir_all(&wasm_parity_dir).expect("create wasm_parity dir");

    write_baseline(&wasm_parity_dir.join("native_baseline.json"), &baseline);
    write_corpus_sql(&wasm_parity_dir.join("corpus_sql.json"), &baseline);

    assert_eq!(
        baseline.len(),
        EXEC_CORPUS.len(),
        "baseline must have one entry per executed mapping"
    );
}

/// Serialize the baseline array WITHOUT the `sql` field (that lives in
/// `corpus_sql.json`); keeps `native_baseline.json` to the digest contract
/// `{name, sql_sha256, row_count, result_sha256}`.
fn write_baseline(path: &Path, baseline: &[serde_json::Value]) {
    let trimmed: Vec<serde_json::Value> = baseline
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e["name"],
                "sql_sha256": e["sql_sha256"],
                "row_count": e["row_count"],
                "result_sha256": e["result_sha256"],
            })
        })
        .collect();
    let json = serde_json::to_string_pretty(&trimmed).expect("serialize baseline");
    std::fs::write(path, format!("{json}\n")).expect("write native_baseline.json");
}

/// Serialize the single-sourced SQL map `{ name: sql }` so `run-parity.mjs`
/// fetches the exact SQL text from the Rust corpus (Task 3 option (a)).
fn write_corpus_sql(path: &Path, baseline: &[serde_json::Value]) {
    let map: serde_json::Map<String, serde_json::Value> = baseline
        .iter()
        .map(|e| (e["name"].as_str().unwrap().to_string(), e["sql"].clone()))
        .collect();
    let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .expect("serialize corpus_sql");
    std::fs::write(path, format!("{json}\n")).expect("write corpus_sql.json");
}
