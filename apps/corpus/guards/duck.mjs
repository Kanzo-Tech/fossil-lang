/**
 * The only thing between these guards and a corpus: the `duckdb` command-line binary.
 *
 * Not a driver, not a binding, not `@duckdb/duckdb-wasm`, and emphatically not any crate of ours.
 * A guard that reads the corpus through the writer's own types proves the types are self-consistent
 * and nothing else — it cannot see a promise the format makes to somebody who is not us. So the
 * checker installs nothing: `node` and `duckdb` on the path, which is what a stranger already has if
 * they are reading Parquet at all.
 *
 * Every query goes over stdin rather than `-c`, because a corpus path is interpolated into SQL and
 * a shell is one more grammar for it to get through.
 */

import { spawnSync } from "node:child_process";

/** Escape a path for a single-quoted DuckDB string literal. */
export function lit(value) {
  return String(value).replace(/'/g, "''");
}

/**
 * The one place a `duckdb` run that did not succeed becomes a sentence.
 *
 * A process that never STARTED reports in `run.error` and leaves `status`, `stdout` and `stderr`
 * null — so `run.stderr.trim()` on that path throws a `TypeError` from inside this file and says
 * nothing at all about the binary that is missing. That is not hypothetical: it is what a release
 * gate reported, `Cannot read properties of null (reading 'trim')` at this module, when the runner
 * had no `duckdb`. Both callers come through here so neither can grow the hole back.
 *
 * @param {import('node:child_process').SpawnSyncReturns<string>} run
 * @param {string} sql
 * @returns {Error | null} the failure, or `null` if there was none
 */
function failure(run, sql) {
  if (run.error) {
    if (run.error.code === "ENOENT") {
      return new Error(
        "the `duckdb` binary is not on PATH. The checker needs it and nothing else; " +
          "see https://duckdb.org/docs/installation.",
      );
    }
    return new Error(`duckdb could not be run: ${run.error.message}\n--- sql ---\n${sql}`, {
      cause: run.error,
    });
  }
  if (run.status !== 0) {
    const said = (run.stderr ?? "").trim();
    const why = run.status === null ? `was killed by ${run.signal}` : `exited ${run.status}`;
    return new Error(`duckdb ${why}\n${said}\n--- sql ---\n${sql}`);
  }
  return null;
}

/**
 * Run one SQL statement and return its rows as objects.
 *
 * @param {string} sql
 * @returns {Array<Record<string, unknown>>}
 */
export function query(sql) {
  const run = spawnSync("duckdb", ["-json", "-noheader", "-batch"], {
    input: `${sql};`,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });

  const bad = failure(run, sql);
  if (bad) throw bad;

  const out = run.stdout.trim();
  if (out === "") return [];
  try {
    return JSON.parse(out);
  } catch (cause) {
    throw new Error(`duckdb did not answer JSON:\n${out.slice(0, 2000)}`, { cause });
  }
}

/** Run one SQL statement expected to answer a single row with a single column. */
export function scalar(sql) {
  const rows = query(sql);
  if (rows.length !== 1) throw new Error(`expected one row, got ${rows.length}\n${sql}`);
  const values = Object.values(rows[0]);
  if (values.length !== 1) throw new Error(`expected one column, got ${values.length}\n${sql}`);
  return values[0];
}

/**
 * Run a statement for its effect. Used only by the fixture writer, never by a guard: a guard that
 * can write is a guard that can repair what it was asked to find.
 */
export function execute(sql) {
  const run = spawnSync("duckdb", ["-batch"], { input: `${sql};`, encoding: "utf8" });
  const bad = failure(run, sql);
  if (bad) throw bad;
}

/** Whether the `duckdb` binary is available, so the caller can say so once instead of per guard. */
export function available() {
  const run = spawnSync("duckdb", ["-version"], { encoding: "utf8" });
  return run.status === 0 ? run.stdout.trim() : null;
}
