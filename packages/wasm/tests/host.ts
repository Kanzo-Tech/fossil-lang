import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

import { initFossilStorage } from '@fossil-lang/storage';
import type { Engine, Host, Scope, Table } from '@fossil-lang/types';
import { vi } from 'vitest';

await initFossilStorage(
  await readFile(createRequire(import.meta.url).resolve('@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm')),
);

export const CONNECTIONS = { vocab: 's3://vocab/shapes', lake: 's3://lake' };

/**
 * A host that vends a read credential on each connection's prefix, and a `fetch` that answers the
 * GET of `person.shex` with `shape` as S3 does — recording every scope asked and every URL fetched.
 */
export function recordingHost(shape: string) {
  const asked: Scope[] = [];
  const fetched: string[] = [];
  const host: Host = {
    connections: async () => CONNECTIONS,
    credentials: async (scope) => {
      asked.push(scope);
      if (!('connection' in scope)) return [];
      const prefix = `${CONNECTIONS[scope.connection as keyof typeof CONNECTIONS]}/`;
      return [
        {
          prefix,
          config: {
            's3.access-key-id': 'K',
            's3.secret-access-key': 'secret',
            's3.endpoint': 'http://minio.example',
            's3.path-style-access': 'true',
            'client.region': 'us-east-1',
          },
        },
      ];
    },
  };
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const { url } = new Request(input, init);
      fetched.push(url);
      const response =
        new URL(url).pathname === '/vocab/shapes/person.shex'
          ? new Response(shape, {
              headers: {
                'content-length': String(new TextEncoder().encode(shape).length),
                'last-modified': 'Tue, 29 Sep 2026 08:00:00 GMT',
                etag: '"e"',
              },
            })
          : new Response('', { status: 404 });
      // A fetched response carries its URL, and a client reads it back.
      return Object.defineProperty(response, 'url', { value: url });
    }),
  );
  return { host, asked, fetched };
}

/** Rows as the columns an engine answers in. */
export function tableOf(rows: readonly Record<string, unknown>[]): Table {
  const names = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  return {
    numRows: rows.length,
    schema: { fields: names.map((name) => ({ name })) },
    getChild: (name) =>
      names.includes(name)
        ? {
            length: rows.length,
            get: (i) => rows[i]?.[name] ?? null,
            toArray: () => rows.map((row) => row[name]),
          }
        : null,
  };
}

/** An engine whose `DESCRIBE` of any source answers `columns`, or throws `failure` when given one. */
export function describingEngine(
  columns: readonly { column_name: string; column_type: string }[],
  failure?: Error,
): Engine {
  return {
    query: async (sql) => {
      if (sql.includes('duckdb_extensions()')) return tableOf([{ loaded: true }]);
      if (!sql.startsWith('DESCRIBE')) return tableOf([]);
      if (failure !== undefined) throw failure;
      return tableOf(columns);
    },
    registerFiles: async () => {},
    dropFiles: async () => {},
  };
}
