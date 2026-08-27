/**
 * What the compiler is told about the inputs.
 *
 * `User := io.csv("users.csv")` declares no columns. Natively `fossil-cli` calls
 * `fossil-introspect`, which runs `DESCRIBE SELECT * FROM read_csv_auto('<url>')` through
 * DuckDB and feeds the answer to the compile. This is that, in the browser, through
 * DuckDB-WASM — the same query against the same reader.
 *
 * **`@fossil-lang/introspect` is the one home for this**, and this file is not trying to
 * be a second one: it is eleven lines of the same idea, kept local because the package's
 * surface takes a resolver and a URL and this app has bytes it already owns. Reaching for
 * the package the moment a second source format appears is the right move; the type table
 * below is where it would stop being defensible, because
 * `packages/introspect/tests/rust-parity.test.ts` derives the real one out of
 * `crates/fossil-introspect/src/lib.rs` and this hand-written copy is not in that test.
 */
import type { InferredDescriptorJson, InferredPrimitive } from '@fossil-lang/wasm';

import { query, register, type QueryRow } from './duckdb.js';

/**
 * DuckDB's declared type → fossil's primitive lattice.
 *
 * Deliberately partial: anything unrecognised becomes `string`, which is what an
 * introspection that cannot tell should say. The authoritative table lives in
 * `crates/fossil-introspect/src/lib.rs`.
 */
function primitiveOf(duckType: string): InferredPrimitive {
  const t = duckType.toUpperCase();
  if (/^(TINYINT|SMALLINT|INTEGER|BIGINT|HUGEINT|UTINYINT|USMALLINT|UINTEGER|UBIGINT)$/.test(t)) return 'integer';
  if (/^(FLOAT|DOUBLE|REAL|DECIMAL)/.test(t)) return 'float';
  if (t === 'BOOLEAN') return 'bool';
  if (t === 'DATE') return 'date';
  if (t.startsWith('TIMESTAMP')) return 'date_time';
  if (t === 'TIME') return 'time';
  return 'string';
}

/**
 * Introspect one CSV the host already holds, as the descriptor the compiler consumes.
 *
 * `uri` is the string the PROGRAM writes — `"users.csv"` — not the name the bytes were
 * staged under. The checker only ever sees what the program says, so a descriptor keyed
 * on the resolved location would never be found.
 */
export async function describeCsv(uri: string, bytes: Uint8Array): Promise<InferredDescriptorJson> {
  await register(uri, bytes);
  const rows = await query(`DESCRIBE SELECT * FROM read_csv_auto('${uri.replace(/'/g, "''")}')`);
  return {
    uri,
    columns: rows.map((row: QueryRow) => ({
      name: String(row['column_name']),
      primitive: primitiveOf(String(row['column_type'])),
    })),
    // The bytes are a build-time import and cannot change under us, so a constant token is
    // honest here. A host reading a real file puts an ETag or a digest in; empty would mean
    // "I cannot tell", which the cache reads as never-fresh and re-introspects every compile.
    freshness_token: `inline:${bytes.byteLength}`,
  };
}
