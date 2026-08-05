import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { beforeAll, describe, expect, it, vi } from 'vitest';

import { createGraphClient, initFossilGraphWasm } from '../src/index.js';
import type { QueryRow } from '../src/index.js';

// Smoke test for @fossil-lang/graph: exercises the REAL fossil-graph-wasm module
// (first runtime test of the W5 step-2 binding) end-to-end — manifest parse +
// verb→SQL + the JS query callback round-trip — WITHOUT a real DuckDB. The
// `query` mock mirrors the in-crate `FakeExec`: `count(*)` → `{ n: 3 }`. A full
// DuckDB-WASM e2e (real SQL execution) is W5 step 3.

const manifestFiles: Record<string, string> = JSON.parse(
  await readFile(fileURLToPath(new URL('./fixtures/manifest.json', import.meta.url)), 'utf8'),
) as Record<string, string>;

// Mirror the crate's FakeExec: schema verbs still issue a `count(*)` to size the
// type, so the callback must answer it. Anything else returns no rows.
const fakeQuery = vi.fn(async (sql: string): Promise<QueryRow[]> => {
  if (sql.includes('count(*)')) return [{ n: 3 }];
  return [];
});

beforeAll(async () => {
  // `--target web` init() defaults to `fetch(url)`, but Node's fetch rejects
  // `file://`. Read the bytes and pass a BufferSource instead — same approach
  // as @fossil-lang/wasm's tests, sidestepping the file://-fetch portability gap.
  const bytes = await readFile(
    fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
  );
  await initFossilGraphWasm({ wasmUrl: bytes as unknown as URL });
});

describe('createGraphClient over fossil-graph-wasm', () => {
  it('schema: reads the manifest + issues count(*) via the query callback', async () => {
    const graph = createGraphClient({ query: fakeQuery, manifestFiles });

    const { vertices, fields } = await graph.schema();

    expect(vertices).toHaveLength(1);
    const person = vertices[0]!;
    expect(person.name).toBe('Person');
    expect(person.iri).toBe('http://example.org/Person');
    expect(person.count).toBe(3);
    // dense_id is a reserved writer column → hidden; user fields surfaced.
    expect(person.fields).toEqual(['age', 'name']);

    // Naming no type costs no per-field query — the point of the collapse.
    expect(fields).toHaveLength(0);

    // The count(*) round-tripped through the JS callback into WASM and back.
    expect(fakeQuery).toHaveBeenCalledWith(expect.stringContaining('count(*)'));
  });

  it('schema: derives the GraphAr edge table name + carries the predicate IRI', async () => {
    const graph = createGraphClient({ query: fakeQuery, manifestFiles });

    const { edges } = await graph.schema();

    expect(edges).toHaveLength(1);
    const knows = edges[0]!;
    expect(knows.source_type).toBe('Person');
    expect(knows.name).toBe('knows');
    expect(knows.target_type).toBe('Person');
    expect(knows.iri).toBe('http://example.org/knows');
    expect(knows.table_name).toBe('Person_knows_Person');
  });

  it('surfaces verb errors from WASM as a rejected promise', async () => {
    const graph = createGraphClient({ query: fakeQuery, manifestFiles });

    await expect(
      graph.schema({ vertex_type: 'Nope', field: 'ghost' }),
    ).rejects.toThrow();
  });
});
