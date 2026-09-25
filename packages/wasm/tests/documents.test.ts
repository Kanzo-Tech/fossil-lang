/**
 * The host flow across the wasm-bindgen boundary: `setConnections`, then
 * `resolveDocuments` over `playground.workspace(handle)`, then `check`. The
 * Rust half is `crates/fossil-wasm/tests/documents.rs`; this covers what it
 * cannot reach — the record crossing in, and the rows crossing out.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { resolveDocuments, type SourceHost } from '@fossil-lang/types';
import { initFossilWasm, FossilPlayground } from '../src/index.js';

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

const CONNECTIONS = { vocab: 'https://minio.example/shapes', lake: 's3://lake' };

describe('FossilPlayground documents and sources', () => {
  it('resolves the documents a program names through the host, keyed as written', async () => {
    const pg = new FossilPlayground();
    try {
      pg.registerInferredDescriptor({
        uri: '@lake/users.csv',
        columns: [
          { name: 'id', primitive: 'string' },
          { name: 'name', primitive: 'string' },
        ],
        freshness_token: '',
      });
      pg.setConnections(CONNECTIONS);
      const handle = pg.openFile('prog.fossil', PROGRAM);
      expect(pg.missingDocuments(handle)).toEqual([
        { key: '@vocab/person.shex', locator: 'https://minio.example/shapes/person.shex' },
      ]);

      const signed: string[] = [];
      const host: SourceHost = {
        connections: async () => CONNECTIONS,
        sign: async (locators) => {
          signed.push(...locators);
          return Object.fromEntries(locators.map((l) => [l, `${l}?signed`]));
        },
      };
      const fetchImpl = (async (url: string) =>
        url === 'https://minio.example/shapes/person.shex?signed'
          ? new Response(DEMANDS_INTEGER)
          : new Response('', { status: 404 })) as typeof fetch;

      const result = await resolveDocuments(pg.workspace(handle), host, fetchImpl);
      expect(result).toEqual({ registered: 1, unread: [] });
      expect(signed).toEqual(['https://minio.example/shapes/person.shex']);
      expect(pg.missingDocuments(handle)).toEqual([]);
      expect(pg.check().some((r) => r.message.includes('expects Integer'))).toBe(true);
    } finally {
      pg.free();
    }
  });

  it('reports the sources a program reads, with the option only where one was written', () => {
    const pg = new FossilPlayground();
    try {
      pg.setConnections(CONNECTIONS);
      const handle = pg.openFile('a/prog.fossil', PROGRAM);
      expect(pg.sources(handle)).toEqual([
        {
          binding: 'users',
          key: '@lake/users.csv',
          locator: 's3://lake/users.csv',
          format: 'csv',
          option: '|',
        },
        { binding: 'orders', key: 'orders.parquet', locator: 'a/orders.parquet', format: 'parquet' },
      ]);
    } finally {
      pg.free();
    }
  });
});
