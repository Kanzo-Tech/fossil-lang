// SC#1 manual cross-engine parity gate (ADR-0014).
//
// Tier 3 of the two-tier-plus-baseline parity strategy:
//   1. snapshot tier  — fossil-codegen/tests/corpus.rs locks the SQL text.
//   2. native tier     — fossil-runtime/tests/corpus_exec.rs runs the executable
//                        corpus subset on native duckdb 1.10502, asserts the
//                        result bytes, and WRITES the digest baseline
//                        crates/fossil-codegen/tests/wasm_parity/native_baseline.json
//                        plus the single-sourced SQL list corpus_sql.json.
//   3. WASM tier (THIS) — re-run the SAME SQL on @duckdb/duckdb-wasm 1.33.x,
//                        recompute the digests with the SAME serialization, and
//                        diff against the baseline. Exit non-zero on any
//                        mismatch. This is the documented MANUAL phase-close
//                        command (NOT wired into cargo test / CI — the 6.4MB MVP
//                        bundle is too heavy for per-PR runs).
//
// VERSION ASYMMETRY (RESEARCH Pitfall 6): native duckdb is 1.10502; DuckDB-WASM
// is 1.33.x. The versions are intentionally different — the baseline + this
// harness are precisely the mechanism that CATCHES any divergence between them.
// The corpus sticks to portable SQL (read_csv_auto, CAST AS VARCHAR, ORDER BY,
// GROUP BY, UNION, DISTINCT ON) so both engines agree.
//
// THE RESULT-SET SERIALIZATION (must match corpus_exec.rs byte-for-byte):
//   - every cell rendered to its UTF-8 text form (NULL -> empty string),
//   - the cells of one row joined with "|" (U+007C),
//   - the rows joined with "\n" (U+000A),
//   then SHA-256'd. row_count is the number of result rows. sql_sha256 is the
//   SHA-256 of the exact SQL text. We verify sql_sha256 FIRST (proves both
//   engines ran identical SQL) before comparing the result digest.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..", "..");
const BASELINE_PATH = resolve(
  ROOT,
  "crates/fossil-codegen/tests/wasm_parity/native_baseline.json",
);
const CORPUS_SQL_PATH = resolve(
  ROOT,
  "crates/fossil-codegen/tests/wasm_parity/corpus_sql.json",
);

// SC#2 (Phase 5): the io/csv + io/json + io/parquet parity tier. Written by
// crates/fossil-runtime/tests/io_parity_corpus.rs (the native side) — a mapping
// reading all three source formats + clean/parse/seq ops (sources end-to-end;
// ops via direct MIR/Expr::Call construction, ADR-0009 reachability gap). This
// harness registers the three fixture inputs in the WASM VFS and re-runs the
// identical SQL, diffing the digests against the io baseline.
const IO_BASELINE_PATH = resolve(
  ROOT,
  "crates/fossil-codegen/tests/wasm_parity/io_parity_baseline.json",
);
const IO_SQL_PATH = resolve(
  ROOT,
  "crates/fossil-codegen/tests/wasm_parity/io_parity_sql.json",
);

// The fixture CSVs the executable corpus reads, by the relative path the SQL
// names them (read_csv_auto('examples/users.csv', ...)). Registered into the
// DuckDB-WASM virtual filesystem under the SAME path so the SQL is verbatim.
const FIXTURES = [
  "examples/users.csv",
  "examples/orders.csv",
  "examples/a.csv",
  "examples/b.csv",
];

// SC#2 io tier: the three source-format fixtures, registered under their
// SQL-referenced relative paths (read_csv_auto('tests/wasm_parity/fixtures/...'),
// read_json_auto(...), read_parquet(...)) so the SQL text is byte-identical
// native↔WASM. The Parquet fixture is a committed, deterministic file (generated
// once via DuckDB COPY) so both engines read the same bytes.
const IO_FIXTURES = [
  "tests/wasm_parity/fixtures/io_people.csv",
  "tests/wasm_parity/fixtures/io_orgs.json",
  "tests/wasm_parity/fixtures/io_depts.parquet",
];

function sha256Hex(s) {
  return createHash("sha256").update(s, "utf8").digest("hex");
}

// Canonical, engine-portable serialization (mirrors corpus_exec.rs).
function serializeResult(rows) {
  return rows
    .map((row) => row.map((cell) => (cell === null || cell === undefined ? "" : String(cell))).join("|"))
    .join("\n");
}

async function makeDb() {
  // The node-BLOCKING build is the simplest functional path in Node: it runs
  // the WASM module in-process (no browser-style Worker with addEventListener,
  // which node:worker_threads lacks). `createDuckDB` takes a bundle of LOCAL
  // file paths from the installed package's dist/.
  const { createRequire } = await import("node:module");
  const req = createRequire(import.meta.url);
  const duckdb = req("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
  const distDir = dirname(req.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));

  const DUCKDB_BUNDLES = {
    mvp: {
      mainModule: resolve(distDir, "duckdb-mvp.wasm"),
      mainWorker: resolve(distDir, "duckdb-node-mvp.worker.cjs"),
    },
    eh: {
      mainModule: resolve(distDir, "duckdb-eh.wasm"),
      mainWorker: resolve(distDir, "duckdb-node-eh.worker.cjs"),
    },
  };

  const logger = new duckdb.ConsoleLogger(duckdb.LogLevel.WARNING);
  const db = await duckdb.createDuckDB(DUCKDB_BUNDLES, logger, duckdb.NODE_RUNTIME);
  await db.instantiate(() => {});
  return db;
}

/// Diff one baseline (`[{name, sql_sha256, row_count, result_sha256}]`) against
/// its `{name: sql}` map, re-running each SQL on the shared DuckDB-WASM
/// connection. Returns the number of failures.
function runTier(conn, tierName, baseline, sqlMap) {
  let failures = 0;
  for (const entry of baseline) {
    const sql = sqlMap[entry.name];
    if (typeof sql !== "string") {
      console.error(`MISSING SQL (${tierName}): no SQL entry for "${entry.name}"`);
      failures += 1;
      continue;
    }

    // (1) Prove both engines ran identical SQL.
    const sqlHash = sha256Hex(sql);
    if (sqlHash !== entry.sql_sha256) {
      console.error(
        `SQL MISMATCH (${tierName}) for ${entry.name}:\n  baseline sql_sha256=${entry.sql_sha256}\n  wasm     sql_sha256=${sqlHash}`,
      );
      failures += 1;
      continue;
    }

    // (2) Execute on DuckDB-WASM and read the ordered result set as text rows.
    const table = conn.query(sql);
    const colNames = table.schema.fields.map((f) => f.name);
    const rows = table.toArray().map((r) => colNames.map((c) => r[c]));

    const rowCount = rows.length;
    const resultHash = sha256Hex(serializeResult(rows));

    // (3) Diff against the native baseline.
    if (rowCount !== entry.row_count || resultHash !== entry.result_sha256) {
      console.error(
        `RESULT MISMATCH (${tierName}) for ${entry.name}:\n` +
          `  baseline row_count=${entry.row_count} result_sha256=${entry.result_sha256}\n` +
          `  wasm     row_count=${rowCount} result_sha256=${resultHash}`,
      );
      failures += 1;
    } else {
      console.log(`OK  [${tierName}] ${entry.name}  (rows=${rowCount})`);
    }
  }
  return failures;
}

async function main() {
  const corpusBaseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
  const corpusSql = JSON.parse(readFileSync(CORPUS_SQL_PATH, "utf8"));

  // The SC#2 io tier is optional at load time only so the corpus tier still runs
  // if the io baseline has not been produced yet; in a full phase-close run the
  // native io_parity_corpus.rs test writes it first.
  let ioBaseline = [];
  let ioSql = {};
  try {
    ioBaseline = JSON.parse(readFileSync(IO_BASELINE_PATH, "utf8"));
    ioSql = JSON.parse(readFileSync(IO_SQL_PATH, "utf8"));
  } catch {
    console.warn(
      "WARN: io_parity_baseline.json not found — run `cargo test -p fossil-runtime --test io_parity_corpus` first to produce the SC#2 io tier.",
    );
  }

  const db = await makeDb();

  // Register the corpus fixture CSVs + the SC#2 io fixtures (csv/json/parquet)
  // into the WASM VFS under their SQL-referenced relative paths (so the SQL text
  // is byte-identical native↔WASM).
  for (const rel of [...FIXTURES, ...IO_FIXTURES]) {
    const buf = readFileSync(resolve(ROOT, rel));
    db.registerFileBuffer(rel, new Uint8Array(buf));
  }

  const conn = db.connect();

  let failures = 0;
  failures += runTier(conn, "corpus", corpusBaseline, corpusSql);
  failures += runTier(conn, "io", ioBaseline, ioSql);

  conn.close();

  const total = corpusBaseline.length + ioBaseline.length;
  if (failures > 0) {
    console.error(`\nPARITY FAILED: ${failures} entr${failures === 1 ? "y" : "ies"} diverged.`);
    process.exit(1);
  }
  console.log(
    `\nPARITY OK: all ${total} entries match (corpus=${corpusBaseline.length}, io=${ioBaseline.length}).`,
  );
  process.exit(0);
}

main().catch((err) => {
  console.error("run-parity.mjs crashed:", err);
  process.exit(1);
});
