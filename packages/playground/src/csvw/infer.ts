/**
 * CSVW descriptor inference via DuckDB-WASM `DESCRIBE SELECT * FROM read_csv_auto(...)`.
 *
 * Foundation for PLAY-09 (editable CSVW preview) + PLAY-11 (automatic inference
 * when the user uploads a raw CSV).
 *
 * Per 09-RESEARCH.md Pattern 2 + 09-CONTEXT.md decisions:
 *   - DuckDB's `read_csv_auto` does the column-name + type inference.
 *   - `DESCRIBE SELECT * ...` returns one row per column (`column_name`, `column_type`).
 *   - We map each DuckDB type to a CSVW XSD short name via `duckdbTypeToCsvw`.
 *
 * Test-friendly: the function accepts a structural `DescribingConnection`
 * interface — a tiny subset of `@duckdb/duckdb-wasm`'s `AsyncDuckDBConnection`.
 * Unit tests pass a hand-rolled mock; integration uses the real connection.
 */

import { duckdbTypeToCsvw } from './type-map.js';

/** A single column entry in a CSVW `tableSchema`. */
export interface CsvwColumn {
  /** Column name as inferred by DuckDB's `read_csv_auto` (from the CSV header). */
  name: string;
  /** CSVW XSD short name (e.g., `"integer"`, `"string"`). */
  datatype: string;
}

/** Minimal CSVW JSON-LD descriptor — `@context` + `url` + `tableSchema.columns`. */
export interface CsvwTable {
  '@context': 'http://www.w3.org/ns/csvw';
  url: string;
  tableSchema: { columns: CsvwColumn[] };
}

/**
 * Structural subset of `AsyncDuckDBConnection` used by `inferCsvw`.
 *
 * The real `AsyncDuckDBConnection.query()` returns an arrow.Table; we only
 * call `.toArray()` on the result, expecting row objects with `column_name`
 * and `column_type` string fields (DuckDB's documented `DESCRIBE` output).
 *
 * Defined as an interface here rather than importing the duckdb type so
 * that the module remains testable from pure-Node Vitest (no DuckDB-WASM
 * load in unit tests).
 */
export interface DescribingConnection {
  query: (sql: string) => Promise<{
    toArray: () => Array<{ column_name: string; column_type: string }>;
  }>;
}

/**
 * Infer a minimal CSVW JSON-LD descriptor for the CSV at `csvUrl`.
 *
 * Runs `DESCRIBE SELECT * FROM read_csv_auto('<csvUrl>')` via the supplied
 * connection, maps each `(column_name, column_type)` row to a `CsvwColumn`,
 * and wraps the result in the CSVW envelope.
 *
 * SQL-injection-safe: the `csvUrl` is escaped per ANSI SQL string-literal
 * rules (`'` → `''`) before being interpolated. Callers MAY still want to
 * validate the URL scheme themselves.
 *
 * @param conn    A connection-like object with a `query(sql)` method.
 * @param csvUrl  CSV URL or path — anything DuckDB's `read_csv_auto` accepts
 *                (HTTP(S), local file, `@examples/foo.csv` after virtual-FS
 *                registration).
 * @returns       Minimal CSVW descriptor with `@context`, `url`, and `tableSchema.columns`.
 */
export async function inferCsvw(
  conn: DescribingConnection,
  csvUrl: string,
): Promise<CsvwTable> {
  // ANSI SQL string-literal escape: double up single quotes.
  const escaped = csvUrl.replace(/'/g, "''");
  const result = await conn.query(
    `DESCRIBE SELECT * FROM read_csv_auto('${escaped}')`,
  );
  const columns: CsvwColumn[] = result.toArray().map((row) => ({
    name: row.column_name,
    datatype: duckdbTypeToCsvw(row.column_type),
  }));
  return {
    '@context': 'http://www.w3.org/ns/csvw',
    url: csvUrl,
    tableSchema: { columns },
  };
}
