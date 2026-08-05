import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ConsoleLogger, NODE_RUNTIME, createDuckDB } from '@duckdb/duckdb-wasm/blocking';
import { beforeAll, describe, expect, it } from 'vitest';

import { createGraphClient, initFossilGraphWasm } from '../src/index.js';
import type { GraphClient, QueryRow } from '../src/index.js';

// End-to-end: the binding's verbs run for real against DuckDB-WASM (the actual
// target engine, blocking node API) reading an in-memory GraphAr-shaped dataset.
// Unlike client.test.ts (mock query), this exercises verb→SQL→DuckDB→rows→WASM
// round-trips with real computed results — the W5 step-3 part-3 validation.

const require = createRequire(import.meta.url);

const manifestFiles: Record<string, string> = JSON.parse(
  await readFile(fileURLToPath(new URL('./fixtures/manifest.json', import.meta.url)), 'utf8'),
) as Record<string, string>;

let graph: GraphClient;

beforeAll(async () => {
  // 1. Boot fossil-graph-wasm (verb→SQL core).
  const wasmBytes = await readFile(
    fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
  );
  await initFossilGraphWasm({ wasmUrl: wasmBytes as unknown as URL });

  // 2. Boot DuckDB-WASM (blocking node bindings — no worker, runs in-process).
  const dist = dirname(require.resolve('@duckdb/duckdb-wasm'));
  const db = await createDuckDB(
    {
      mvp: {
        mainModule: resolve(dist, './duckdb-mvp.wasm'),
        mainWorker: resolve(dist, './duckdb-node-mvp.worker.cjs'),
      },
      eh: {
        mainModule: resolve(dist, './duckdb-eh.wasm'),
        mainWorker: resolve(dist, './duckdb-node-eh.worker.cjs'),
      },
    },
    new ConsoleLogger(),
    NODE_RUNTIME,
  );
  await db.instantiate();
  const conn = db.connect();

  // GraphAr-shaped tables the verb SQL targets (`FROM "Person"` etc.). The
  // vertex carries the reserved `dense_id` + `subject` plus user fields; the
  // edge carries resolved src/dst dense ids — mirrors the W0b writer output.
  conn.query(`
    CREATE TABLE "Person" AS SELECT * FROM (VALUES
      (0, 'http://example.org/p/alice', 30, 'Alice'),
      (1, 'http://example.org/p/bob',   41, 'Bob'),
      (2, 'http://example.org/p/carol', 25, 'Carol')
    ) t(dense_id, subject, age, name);
  `);
  conn.query(`
    CREATE TABLE "Person_knows_Person" AS SELECT * FROM (VALUES
      (0, 1), (1, 2)
    ) e(src_dense, dst_dense);
  `);

  // The host query callback: run SQL on DuckDB, return rows as plain objects.
  // BIGINT/UBIGINT come back as JS BigInt — coerce to Number (counts are small)
  // so they deserialise cleanly back into the WASM verb logic.
  const query = async (sql: string): Promise<QueryRow[]> => {
    const table = conn.query(sql);
    return table.toArray().map((row: { toJSON(): Record<string, unknown> }) => {
      const obj = row.toJSON();
      for (const [k, v] of Object.entries(obj)) {
        if (typeof v === 'bigint') obj[k] = Number(v);
      }
      return obj;
    });
  };

  graph = createGraphClient({ query, manifestFiles });
});

describe('@fossil-lang/graph verbs e2e against DuckDB-WASM', () => {
  it('schema: real count(*) from the loaded table', async () => {
    const { vertices } = await graph.schema();
    expect(vertices).toHaveLength(1);
    expect(vertices[0]!.count).toBe(3);
    expect(vertices[0]!.fields).toEqual(['age', 'name']);
  });

  it('aggregate: count grouped by a user field', async () => {
    const { rows } = await graph.aggregate({
      vertex_type: 'Person',
      agg: 'count',
      group_by: 'name',
    });
    // 3 distinct names → 3 groups, each count 1.
    expect(rows).toHaveLength(3);
    expect(rows.every((r) => r.value === 1)).toBe(true);
  });

  it('aggregate: bins real ages over ranges', async () => {
    const { rows, edges } = await graph.aggregate({
      vertex_type: 'Person',
      group_by: 'age',
      agg: 'count',
      bins: 4,
    });
    expect(edges).toHaveLength(5); // bins + 1
    expect(rows).toHaveLength(4); // dense: one row per bin
    // Every age (25, 30, 41) lands in some bin; the counts sum to 3.
    expect(rows.reduce((a, r) => a + r.value, 0)).toBe(3);
  });

  it('read: descending order_by returns the real ordering', async () => {
    const { rows } = (await graph.read({
      vertex_type: 'Person',
      order_by: 'age',
      descending: true,
      limit: 2,
    })) as { rows: Array<Record<string, unknown>> };
    expect(rows).toHaveLength(2);
    expect(rows[0]!.name).toBe('Bob'); // age 41
    expect(rows[1]!.name).toBe('Alice'); // age 30
  });

  it('schema: a named field carries distinct + datatype + samples', async () => {
    const { fields } = await graph.schema({ vertex_type: 'Person', field: 'age' });
    expect(fields).toHaveLength(1);
    expect(fields[0]!.datatype).toBe('int64');
    expect(fields[0]!.distinct).toBe(3);
    expect(fields[0]!.samples).toHaveLength(3);
  });

  it('execute_sql: escape hatch returns columns + rows', async () => {
    const r = await graph.executeSql({
      sql: 'SELECT name FROM "Person" ORDER BY age DESC',
    });
    expect(r.columns.map((c) => c.name)).toContain('name');
    expect(r.rows).toHaveLength(3);
  });
});
