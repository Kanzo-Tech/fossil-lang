//! SC#2 — io/csv + io/json + io/parquet native↔WASM parity (the io tier of
//! ADR-0014's cross-engine parity strategy).
//!
//! NATIVE-ONLY (`fossil-runtime` carries a `wasm32` `compile_error!` tripwire).
//!
//! # Honest discharge mode (ADR-0009 reachability gap — W-label)
//!
//! This file discharges SC#2 in TWO explicitly-separated parts; do NOT read it
//! as a single "a surface `seq.map` pipeline compiles" claim:
//!
//!   (a) **io/csv + io/json + io/parquet SOURCE READ** is genuinely END-TO-END:
//!       a [`MirGraph`] reading all three formats via the 05-04 `Op::Source`
//!       arms is run through the REAL `fossil_codegen::codegen_graph_for_test`
//!       and the compiler-emitted SQL is asserted to carry `read_csv_auto` /
//!       `read_json_auto` / `read_parquet`. Sources ARE surface-reachable, so
//!       this is a true compile path.
//!   (b) **seq/clean/parse OPERATIONS** are exercised via DIRECT CONSTRUCTION
//!       (`Expr::Call { func: "clean.trim" }`, `Expr::Call { func:
//!       "parse.integer" }`, a `Op::Union` seq op) — there is NO surface
//!       pipeline / call syntax in v0.1 (ADR-0009), so these are NOT
//!       type-checked end-to-end in Phase 5; surface syntax is deferred to
//!       Phase 6+. The compiler-emitted SQL is asserted to carry the 05-02
//!       registry forms (`trim(...)`, `CAST(... AS BIGINT)`).
//!
//! # The parity contract (same shape as `corpus_exec.rs`)
//!
//! The native side writes `io_parity_baseline.json` + `io_parity_sql.json` next
//! to the Phase-4 corpus baseline (`crates/fossil-codegen/tests/wasm_parity/`).
//! `tests/wasm_parity/run-parity.mjs` registers the three fixture inputs into
//! the DuckDB-WASM VFS (`registerFileBuffer`) under the same SQL-referenced
//! relative paths, re-runs the identical SQL on `@duckdb/duckdb-wasm`, and
//! diffs the digests. The result-set serialization is byte-for-byte identical
//! to `corpus_exec.rs`: cells `|`-joined, rows `\n`-joined, SHA-256'd; every
//! projected column is `CAST(... AS VARCHAR)` so the two engines render
//! identically.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::elidable_lifetime_names)]
// The doc strings + fixture comments embed SQL fragments (DuckDB fn names), not
// Rust items.
#![allow(clippy::literal_string_with_formatting_args, clippy::doc_markdown)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use duckdb::Connection;
use fossil_codegen::codegen_graph_for_test;
use fossil_hir::{Primitive, Record, RecordField, Ty, TyKind};
use fossil_mir::graph::MirGraph;
use fossil_mir::op::{Expr, Op, SourceFormat};
use sha2::{Digest, Sha256};
use smol_str::SmolStr;

/// The three fixture inputs, by the relative path the SQL names them. Registered
/// into the DuckDB-WASM VFS under the SAME paths so the SQL text is byte-identical
/// native↔WASM (the `run-parity.mjs` `FIXTURES` list mirrors this).
const CSV_URI: &str = "tests/wasm_parity/fixtures/io_people.csv";
const JSON_URI: &str = "tests/wasm_parity/fixtures/io_orgs.json";
const PARQUET_URI: &str = "tests/wasm_parity/fixtures/io_depts.parquet";

fn db() -> fossil_base::FossilDb {
    let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem);
    fossil_base::FossilDb::new(system)
}

fn string_record<'db>(db: &'db dyn fossil_base::Db, names: &[&str]) -> Ty<'db> {
    let string_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let fields: Vec<RecordField<'db>> = names
        .iter()
        .map(|n| RecordField {
            name: SmolStr::from(*n),
            ty: string_ty,
        })
        .collect();
    Ty::new(db, TyKind::Record(Record::new(db, fields)))
}

fn colref(source: &str, column: &str) -> Expr<'static> {
    Expr::ColRef {
        source: SmolStr::from(source),
        column: SmolStr::from(column),
    }
}

/// A terminating `TripleEmit(subject=iri, predicate, object) → Sink` over
/// `input`. Appended so each graph renders the full `CREATE VIEW` + `COPY`
/// script codegen produces in practice (an Extend collapses into the COPY's
/// inner SELECT only when consumed by an emit).
fn emit_and_sink<'db>(input: usize, obj_src: &str, obj_col: &str) -> Vec<Op<'db>> {
    vec![
        Op::TripleEmit {
            input,
            subject: colref(obj_src, "id"),
            predicate: SmolStr::new_static("http://example.org/p"),
            object: colref(obj_src, obj_col),
            graph: None,
        },
        Op::Sink {
            input: input + 1,
            sink: fossil_mir::op::SinkRef::GraphAr,
        },
    ]
}

/// Build the SC#2 `MirGraph` selected by `case`. Each is a terminating
/// `... → TripleEmit → Sink` pipeline so the relevant fragment renders into the
/// emitted SQL:
///
/// - 0: io/csv → `clean.trim` Extend → `parse.integer` Extend → emit/sink.
///   Proves `read_csv_auto` + the 05-02 registry forms (`trim(...)`,
///   `CAST(... AS BIGINT)`) — the OPERATIONS, direct-construction (ADR-0009).
/// - 1: io/json → emit/sink. Proves `read_json_auto` (SOURCE READ).
/// - 2: io/parquet → emit/sink. Proves `read_parquet` (SOURCE READ).
/// - 3: io/csv ∪ io/csv (Union seq op) → emit/sink. Proves a seq-family op
///   participates in codegen (direct construction).
fn io_ops<'db>(db: &'db dyn fossil_base::Db, case: u8) -> Vec<Op<'db>> {
    let str_ty = Ty::new(db, TyKind::Primitive(Primitive::String));
    let int_ty = Ty::new(db, TyKind::Primitive(Primitive::Integer));
    match case {
        0 => {
            let mut ops = vec![
                Op::Source {
                    uri: SmolStr::from(CSV_URI),
                    format: SourceFormat::Csv,
                    row_type: string_record(db, &["id", "name", "age"]),
                },
                // clean.trim(name) — pure_sql Builtin → `trim(...)` (05-02).
                Op::Extend {
                    input: 0,
                    field: SmolStr::new_static("name_clean"),
                    expr: Expr::Call {
                        func: SmolStr::new_static("clean.trim"),
                        args: vec![colref("io_people", "name")],
                        ty: str_ty,
                    },
                },
                // parse.integer(age) — pure_sql Inline::Cast → `CAST(... AS BIGINT)`.
                Op::Extend {
                    input: 1,
                    field: SmolStr::new_static("age_int"),
                    expr: Expr::Call {
                        func: SmolStr::new_static("parse.integer"),
                        args: vec![colref("io_people", "age")],
                        ty: int_ty,
                    },
                },
            ];
            // The emit references BOTH buffered Extends by bare ColRef (the
            // shared substitution path) so codegen inlines both registry forms
            // into the COPY's inner SELECT: subject → the trim'd name, object →
            // the CAST'd age.
            ops.push(Op::TripleEmit {
                input: 2,
                subject: Expr::ColRef {
                    source: SmolStr::default(),
                    column: SmolStr::new_static("name_clean"),
                },
                predicate: SmolStr::new_static("http://example.org/age"),
                object: Expr::ColRef {
                    source: SmolStr::default(),
                    column: SmolStr::new_static("age_int"),
                },
                graph: None,
            });
            ops.push(Op::Sink {
                input: 3,
                sink: fossil_mir::op::SinkRef::GraphAr,
            });
            ops
        }
        1 => {
            let mut ops = vec![Op::Source {
                uri: SmolStr::from(JSON_URI),
                format: SourceFormat::Json,
                row_type: string_record(db, &["id", "org"]),
            }];
            ops.extend(emit_and_sink(0, "io_orgs", "org"));
            ops
        }
        2 => {
            let mut ops = vec![Op::Source {
                uri: SmolStr::from(PARQUET_URI),
                format: SourceFormat::Parquet,
                row_type: string_record(db, &["id", "dept"]),
            }];
            ops.extend(emit_and_sink(0, "io_depts", "dept"));
            ops
        }
        3 => {
            let mut ops = vec![
                Op::Source {
                    uri: SmolStr::from(CSV_URI),
                    format: SourceFormat::Csv,
                    row_type: string_record(db, &["id", "name", "age"]),
                },
                Op::Source {
                    uri: SmolStr::from(CSV_URI),
                    format: SourceFormat::Csv,
                    row_type: string_record(db, &["id", "name", "age"]),
                },
                // a seq op (Op::Union) — direct construction (ADR-0009).
                Op::Union { left: 0, right: 1 },
            ];
            ops.extend(emit_and_sink(2, "step_2", "name"));
            ops
        }
        other => panic!("unknown io case {other}"),
    }
}

/// One parity entry: name, the SQL (reads the three fixtures, ends in a
/// deterministic `ORDER BY`, projects only VARCHAR-cast columns), and the
/// fully-expected ordered result set.
struct IoEntry {
    name: &'static str,
    sql: String,
    expected_rows: Vec<Vec<String>>,
}

/// The SC#2 parity SELECT: read all three formats, JOIN on `id`, apply the SAME
/// `trim(...)` + `CAST(... AS BIGINT)` forms the compiler emits (asserted
/// separately against `codegen_graph_for_test`), and `ORDER BY id`. Every column
/// is `CAST(... AS VARCHAR)` so the result serialization is engine-portable.
///
/// The reader fragments (`read_csv_auto(..., sample_size=-1)` / `read_json_auto`
/// / `read_parquet`) and the op forms (`trim` / `CAST AS BIGINT`) are the exact
/// strings `fossil-codegen` produces — see [`assert_compiler_emits_io_forms`].
fn io_parity_entry() -> IoEntry {
    let sql = format!(
        "SELECT \
           CAST(c.id AS VARCHAR) AS id, \
           CAST(trim(c.name) AS VARCHAR) AS name, \
           CAST(CAST(c.age AS BIGINT) AS VARCHAR) AS age, \
           CAST(j.org AS VARCHAR) AS org, \
           CAST(p.dept AS VARCHAR) AS dept \
         FROM read_csv_auto('{CSV_URI}', sample_size=-1) AS c \
         JOIN read_json_auto('{JSON_URI}') AS j ON c.id = j.id \
         JOIN read_parquet('{PARQUET_URI}') AS p ON c.id = p.id \
         ORDER BY c.id"
    );
    IoEntry {
        name: "io_three_format_read_clean_parse",
        sql,
        expected_rows: vec![
            row(&["1", "Alice", "30", "Acme", "IT"]),
            row(&["2", "Bob", "25", "Globex", "Sales"]),
            row(&["3", "Carol", "40", "Initech", "Eng"]),
        ],
    }
}

fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|c| (*c).to_string()).collect()
}

/// Locate the workspace root (the dir holding `tests/wasm_parity/fixtures/`).
fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir
            .join("tests/wasm_parity/fixtures/io_people.csv")
            .exists()
        {
            return dir;
        }
        assert!(dir.pop(), "could not locate workspace root (io fixtures)");
    }
}

fn run_query(conn: &Connection, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = conn.prepare(sql).expect("prepare io parity SQL");
    let mut rows = stmt.query([]).expect("execute io parity SQL");
    let mut out = Vec::new();
    while let Some(row) = rows.next().expect("read row") {
        let mut cells = Vec::new();
        let mut i = 0;
        while let Ok(cell) = row.get::<usize, Option<String>>(i) {
            cells.push(cell.unwrap_or_default());
            i += 1;
        }
        out.push(cells);
    }
    out
}

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

/// A salsa input selecting which SC#2 `MirGraph` [`io_codegen_in_frame`] builds.
#[salsa::input]
struct IoCase {
    case: u8,
}

/// The codegen-emitted SQL for one SC#2 case, returned out of the tracked frame
/// so the string assertions can run in plain test code.
#[salsa::tracked]
struct IoGenerated<'db> {
    #[returns(ref)]
    sql: String,
}

/// Build the chosen SC#2 `MirGraph` and run the REAL `codegen_graph_for_test`
/// INSIDE a tracked frame (so `MirGraph::new` / `Ty` interning are legal),
/// returning the emitted SQL.
#[salsa::tracked]
fn io_codegen_in_frame<'db>(db: &'db dyn fossil_base::Db, c: IoCase) -> IoGenerated<'db> {
    let mir = MirGraph::new(db, io_ops(db, c.case(db)));
    let plan = codegen_graph_for_test(db, mir);
    IoGenerated::new(db, plan.sql(db).clone())
}

fn case_sql(db: &fossil_base::FossilDb, case: u8) -> String {
    let c = IoCase::new(db, case);
    io_codegen_in_frame(db, c).sql(db).clone()
}

/// Part (a)+(b): the compiler genuinely lowers the 3-format read + the
/// direct-constructed clean/parse/seq ops. Run each SC#2 `MirGraph` through the
/// REAL `codegen_graph_for_test` and assert the emitted SQL carries the relevant
/// reader (sources are surface-reachable — true e2e) and the 05-02 registry
/// forms (ops via direct construction — ADR-0009).
fn assert_compiler_emits_io_forms() {
    let db = db();

    // (a) SOURCE READ — each reader present (the e2e, surface-reachable part).
    let csv_sql = case_sql(&db, 0);
    assert!(
        csv_sql.contains("read_csv_auto("),
        "compiler must emit read_csv_auto for io/csv:\n{csv_sql}"
    );
    let json_sql = case_sql(&db, 1);
    assert!(
        json_sql.contains("read_json_auto("),
        "compiler must emit read_json_auto for io/json:\n{json_sql}"
    );
    let parquet_sql = case_sql(&db, 2);
    assert!(
        parquet_sql.contains("read_parquet("),
        "compiler must emit read_parquet for io/parquet:\n{parquet_sql}"
    );

    // (b) OPERATIONS — the 05-02 registry forms for the direct-constructed Calls
    //     (case 0: clean.trim + parse.integer) and the seq Union (case 3).
    let csv_upper = csv_sql.to_uppercase();
    assert!(
        csv_upper.contains("TRIM("),
        "clean.trim must lower to a DuckDB trim(...) builtin (05-02):\n{csv_sql}"
    );
    assert!(
        csv_upper.contains("CAST(") && csv_upper.contains("AS BIGINT"),
        "parse.integer must lower to CAST(... AS BIGINT) (05-02):\n{csv_sql}"
    );
    let union_sql = case_sql(&db, 3);
    assert!(
        union_sql.to_uppercase().contains("UNION"),
        "the seq Op::Union must lower to a UNION:\n{union_sql}"
    );
}

#[test]
fn io_parity_native_and_write_baseline() {
    // (1) Prove the compiler lowers the 3-format read + the ops (parts a + b).
    assert_compiler_emits_io_forms();

    // (2) Run the parity SELECT on native DuckDB from the workspace root so the
    //     relative fixture paths resolve, and write the io baseline.
    let root = workspace_root();
    std::env::set_current_dir(&root).expect("chdir to workspace root");

    let conn = Connection::open_in_memory().expect("open in-memory duckdb");

    let entry = io_parity_entry();
    let rows = run_query(&conn, &entry.sql);
    assert_eq!(
        rows, entry.expected_rows,
        "native result mismatch for io parity entry {}",
        entry.name
    );

    let serialized = serialize_result(&rows);
    let baseline = vec![serde_json::json!({
        "name": entry.name,
        "sql": entry.sql,
        "sql_sha256": sha256_hex(&entry.sql),
        "row_count": rows.len(),
        "result_sha256": sha256_hex(&serialized),
    })];

    let wasm_parity_dir = root.join("crates/fossil-codegen/tests/wasm_parity");
    std::fs::create_dir_all(&wasm_parity_dir).expect("create wasm_parity dir");
    write_baseline(&wasm_parity_dir.join("io_parity_baseline.json"), &baseline);
    write_sql(&wasm_parity_dir.join("io_parity_sql.json"), &baseline);
}

/// `{name, sql_sha256, row_count, result_sha256}` — the digest contract (drops
/// the `sql` field, which lives in `io_parity_sql.json`).
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
    let json = serde_json::to_string_pretty(&trimmed).expect("serialize io baseline");
    std::fs::write(path, format!("{json}\n")).expect("write io_parity_baseline.json");
}

/// `{ name: sql }` — the single-sourced SQL map the WASM harness fetches so the
/// SQL text is byte-identical native↔WASM.
fn write_sql(path: &Path, baseline: &[serde_json::Value]) {
    let map: serde_json::Map<String, serde_json::Value> = baseline
        .iter()
        .map(|e| (e["name"].as_str().unwrap().to_string(), e["sql"].clone()))
        .collect();
    let json =
        serde_json::to_string_pretty(&serde_json::Value::Object(map)).expect("serialize io sql");
    std::fs::write(path, format!("{json}\n")).expect("write io_parity_sql.json");
}
