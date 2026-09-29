import type { Access, Engine, Host, Scope, StorageCredential, Table } from '@fossil-lang/types';

export const s3 = (prefix: string, key: string, expiresAtMs?: number): StorageCredential => ({
  prefix,
  config: {
    's3.access-key-id': key,
    's3.secret-access-key': 'secret',
    's3.session-token': `token-${key}`,
    's3.endpoint': 'http://localhost:9000',
    's3.path-style-access': 'true',
    'client.region': 'us-east-1',
    ...(expiresAtMs === undefined ? {} : { 's3.session-token-expires-at-ms': String(expiresAtMs) }),
  },
});

export const azure = (prefix: string, sas: string): StorageCredential => ({
  prefix,
  config: { 'adls.sas-token.acct.dfs.core.windows.net': sas },
});

/** A one-row, one-column answer, or an empty one — the shape an engine answers in. */
export function table(column: string, values: readonly unknown[]): Table {
  return {
    numRows: values.length,
    schema: { fields: [{ name: column }] },
    getChild: (name) =>
      name === column
        ? { length: values.length, get: (i) => values[i] ?? null, toArray: () => values }
        : null,
  };
}

/** An engine that records what it was asked and answers `httpfs` as `loaded`. */
export function recordingEngine(loaded = true) {
  const sql: string[] = [];
  const leases: Record<string, string>[] = [];
  const dropped: string[][] = [];
  const engine: Engine = {
    async query(text) {
      if (text.includes('duckdb_extensions()')) return table('loaded', [loaded]);
      sql.push(text);
      return table('Success', []);
    },
    async lend(files) {
      leases.push(files);
    },
    async drop(names) {
      dropped.push([...names]);
    },
  };
  return { engine, sql, leases, dropped };
}

/** A host that vends whatever `vend` answers, and counts the asks. */
export function countingHost(vend: (scope: Scope, access: Access) => StorageCredential[]) {
  const asks: { scope: Scope; access: Access }[] = [];
  const host: Host = {
    connections: async () => ({}),
    credentials: async (scope, access) => {
      asks.push({ scope, access });
      return vend(scope, access);
    },
  };
  return { host, asks };
}
