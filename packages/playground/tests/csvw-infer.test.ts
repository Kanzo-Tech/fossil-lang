/**
 * Tests for inferCsvw — DuckDB DESCRIBE → minimal CSVW JSON-LD descriptor.
 *
 * Uses a hand-rolled mock conforming to the `DescribingConnection` structural
 * type. No DuckDB-WASM boot — tests run in pure Node/happy-dom Vitest.
 *
 * Foundation for PLAY-09 (editable CSVW preview) + PLAY-11 (auto-infer on
 * raw CSV upload).
 *
 * Per 09-04-PLAN Task 3 + 09-RESEARCH.md Pattern 2.
 */

import { describe, test, expect } from 'vitest';
import {
  inferCsvw,
  type DescribingConnection,
} from '../src/csvw/index.js';

/** Build a DescribingConnection that returns the supplied DESCRIBE rows. */
function mockConn(
  rows: Array<{ column_name: string; column_type: string }>,
): DescribingConnection {
  return {
    query: async (_sql: string) => ({ toArray: () => rows }),
  };
}

describe('inferCsvw (PLAY-09/PLAY-11)', () => {
  test('produces minimal CSVW JSON-LD for a typical CSV', async () => {
    const conn = mockConn([
      { column_name: 'id', column_type: 'INTEGER' },
      { column_name: 'name', column_type: 'VARCHAR' },
      { column_name: 'created_at', column_type: 'TIMESTAMP' },
    ]);
    const csvw = await inferCsvw(conn, '@examples/hello.csv');
    expect(csvw).toEqual({
      '@context': 'http://www.w3.org/ns/csvw',
      url: '@examples/hello.csv',
      tableSchema: {
        columns: [
          { name: 'id', datatype: 'integer' },
          { name: 'name', datatype: 'string' },
          { name: 'created_at', datatype: 'dateTime' },
        ],
      },
    });
  });

  test('escapes single quotes in csvUrl (SQL injection-safe)', async () => {
    let receivedSql = '';
    const conn: DescribingConnection = {
      query: async (sql: string) => {
        receivedSql = sql;
        return { toArray: () => [] };
      },
    };
    await inferCsvw(conn, "weird'name.csv");
    // SQL string-literal escape doubles the single quote.
    expect(receivedSql).toContain("weird''name.csv");
    // And the produced SQL is the documented DESCRIBE form.
    expect(receivedSql).toMatch(
      /^DESCRIBE SELECT \* FROM read_csv_auto\('.*'\)$/,
    );
  });

  test('empty CSV → empty columns array', async () => {
    const conn = mockConn([]);
    const csvw = await inferCsvw(conn, 'empty.csv');
    expect(csvw.tableSchema.columns).toEqual([]);
    expect(csvw['@context']).toBe('http://www.w3.org/ns/csvw');
    expect(csvw.url).toBe('empty.csv');
  });

  test('parameterized DECIMAL types map to "decimal"', async () => {
    const conn = mockConn([
      { column_name: 'price', column_type: 'DECIMAL(10,2)' },
      { column_name: 'qty', column_type: 'BIGINT' },
    ]);
    const csvw = await inferCsvw(conn, 'orders.csv');
    expect(csvw.tableSchema.columns).toEqual([
      { name: 'price', datatype: 'decimal' },
      { name: 'qty', datatype: 'integer' },
    ]);
  });
});
