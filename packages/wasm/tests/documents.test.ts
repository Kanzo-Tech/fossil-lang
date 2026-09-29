/**
 * The host flow across the wasm-bindgen boundary: `setConnections`, then
 * `resolveDocuments` over `workspace.workspace(handle)`, then `check`. The
 * Rust half is `crates/fossil-wasm/tests/documents.rs`; this covers what it
 * cannot reach — the record crossing in, and the rows crossing out.
 */
import { describe, it, expect, beforeAll, afterEach, vi } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { resolveDocuments } from '@fossil-lang/storage';
import { initFossilWasm, FossilWorkspace } from '../src/index.js';
import { CONNECTIONS, recordingHost } from './host.js';

beforeAll(async () => {
  const wasmPath = fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url));
  const bytes = await readFile(wasmPath);
  await initFossilWasm(bytes);
});

const PROGRAM = `type { Person } := io.shex("@vocab/person.shex")
users := io.csv("@lake/users.csv", delimiter = "|")
orders := io.parquet("orders.parquet")
User : Person from users
    @subject = "http://example.org/u/{users.id}"
    name = users.name
`;

const DEMANDS_INTEGER = JSON.stringify({
  '@context': 'http://www.w3.org/ns/shex.jsonld',
  type: 'Schema',
  shapes: [
    {
      type: 'ShapeDecl',
      id: 'http://example.org/Person',
      shapeExpr: {
        type: 'Shape',
        expression: {
          type: 'TripleConstraint',
          predicate: 'http://example.org/name',
          valueExpr: { type: 'NodeConstraint', datatype: 'http://www.w3.org/2001/XMLSchema#integer' },
        },
      },
    },
  ],
});

afterEach(() => vi.unstubAllGlobals());

describe('FossilWorkspace documents and sources', () => {
  it('resolves the documents a program names through the host, keyed as written', async () => {
    const ws = new FossilWorkspace();
    try {
      ws.registerInferredDescriptor({
        uri: '@lake/users.csv',
        columns: [
          { name: 'id', primitive: 'string' },
          { name: 'name', primitive: 'string' },
        ],
        freshness_token: '',
      });
      ws.setConnections(CONNECTIONS);
      const handle = ws.openFile('prog.fossil', PROGRAM);
      expect(ws.missingDocuments(handle)).toEqual([
        { key: '@vocab/person.shex', locator: 's3://vocab/shapes/person.shex', connection: 'vocab' },
      ]);

      const { host, asked, fetched } = recordingHost(DEMANDS_INTEGER);
      const result = await resolveDocuments(ws.workspace(handle), host);
      expect(result).toEqual({ registered: 1, unread: [] });
      expect(asked).toEqual([{ connection: 'vocab' }]);
      expect(fetched).toHaveLength(1);
      expect(fetched[0]).toBe('http://minio.example/vocab/shapes/person.shex');
      expect(ws.missingDocuments(handle)).toEqual([]);
      expect(ws.check().some((r) => r.message.includes('expects Integer'))).toBe(true);
    } finally {
      ws.free();
    }
  });

  it('reports the sources a program reads, with the option only where one was written', () => {
    const ws = new FossilWorkspace();
    try {
      ws.setConnections(CONNECTIONS);
      const handle = ws.openFile('a/prog.fossil', PROGRAM);
      expect(ws.sources(handle)).toEqual([
        {
          binding: 'users',
          key: '@lake/users.csv',
          locator: 's3://lake/users.csv',
          connection: 'lake',
          format: 'csv',
          option: '|',
        },
        { binding: 'orders', key: 'orders.parquet', locator: 'a/orders.parquet', format: 'parquet' },
      ]);
    } finally {
      ws.free();
    }
  });
});
