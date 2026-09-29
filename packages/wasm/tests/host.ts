import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

import { initStorage } from '@fossil-lang/storage';
import type { Host, Scope } from '@fossil-lang/types';
import { vi } from 'vitest';

await initStorage(
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
