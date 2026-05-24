/**
 * transformSql unit tests — pure-function coverage of the three transforms
 * the runPipeline composes:
 *
 *   - extractSourceRefs  (find `@connector/path` literals in `read_*(...)`)
 *   - rewriteCopyToCreateTable (COPY ... TO 'X.parquet' → CREATE OR REPLACE TABLE)
 *   - transformSql       (the full end-to-end: resolve + substitute + rewrite)
 *
 * No WASM, no DuckDB, no Worker — these tests are deliberately fast and
 * deterministic so a `pnpm --filter @fossil-lang/playground test` invocation
 * stays under a couple of seconds.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import type { ConnectionResolver } from '@fossil-lang/types';
import {
  extractSourceRefs,
  rewriteCopyToCreateTable,
  transformSql,
} from '../src/run/transformSql.js';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('extractSourceRefs', () => {
  it('finds an @connector/path literal inside read_csv_auto(...)', () => {
    const sql =
      "CREATE VIEW users AS\nSELECT * FROM read_csv_auto('@examples/hello.csv', sample_size=-1);";
    const refs = extractSourceRefs(sql);
    expect(refs).toHaveLength(1);
    expect(refs[0]?.ref.raw).toBe('@examples/hello.csv');
    expect(refs[0]?.ref.connector).toBe('examples');
    expect(refs[0]?.ref.path).toBe('hello.csv');
    expect(refs[0]?.literal).toBe('@examples/hello.csv');
  });

  it('finds refs in read_json / read_parquet too', () => {
    const sql = `
      CREATE VIEW a AS SELECT * FROM read_json('@conn/foo.json');
      CREATE VIEW b AS SELECT * FROM read_parquet('@conn/bar.parquet');
    `;
    const refs = extractSourceRefs(sql);
    expect(refs.map((r) => r.literal)).toEqual([
      '@conn/foo.json',
      '@conn/bar.parquet',
    ]);
  });

  it('skips literals that do NOT start with @ (already-substituted URLs)', () => {
    const sql =
      "SELECT * FROM read_csv_auto('blob:fake-url-12345'); SELECT * FROM read_csv_auto('/local/path.csv');";
    expect(extractSourceRefs(sql)).toEqual([]);
  });

  it('skips malformed @ literals without crashing the caller', () => {
    // No `/` — parseSourceRef throws; extractor swallows + continues.
    const sql = "SELECT * FROM read_csv_auto('@nope-no-slash');";
    expect(extractSourceRefs(sql)).toEqual([]);
  });

  it('finds the same literal multiple times when it appears in multiple readers', () => {
    const sql = `
      CREATE VIEW a AS SELECT * FROM read_csv_auto('@examples/x.csv');
      CREATE VIEW b AS SELECT * FROM read_csv_auto('@examples/x.csv');
    `;
    const refs = extractSourceRefs(sql);
    expect(refs).toHaveLength(2);
    expect(refs[0]?.literal).toBe('@examples/x.csv');
  });
});

describe('rewriteCopyToCreateTable', () => {
  it('rewrites a single-chunk vertex COPY into CREATE OR REPLACE TABLE', () => {
    const sql = `
      COPY (
          SELECT * EXCLUDE (_rn) FROM (
      SELECT *, row_number() OVER (ORDER BY id) AS _rn FROM (SELECT iri AS id, name FROM people) AS _src
          ) AS _chunked
          WHERE _rn BETWEEN 1 AND 1024
      ) TO 'vertex/Person/chunk0.parquet' (FORMAT PARQUET);
    `;
    const { rewrittenSql, vertexTables, edgeTables, tripleTables } =
      rewriteCopyToCreateTable(sql);
    expect(vertexTables).toEqual(['Person']);
    expect(edgeTables).toEqual([]);
    expect(tripleTables).toEqual([]);
    expect(rewrittenSql).toMatch(/CREATE OR REPLACE TABLE "Person" AS/);
    // The inner SELECT body survives the rewrite.
    expect(rewrittenSql).toMatch(/SELECT \* EXCLUDE \(_rn\)/);
    // The COPY/TO/PARQUET artefact is gone.
    expect(rewrittenSql).not.toMatch(/COPY\s*\(/i);
    expect(rewrittenSql).not.toMatch(/FORMAT PARQUET/);
  });

  it('rewrites an AcceptAll output.parquet COPY into CREATE OR REPLACE TABLE "output" classified as triple', () => {
    const sql = `
      CREATE VIEW users AS
      SELECT * FROM read_csv_auto('@examples/hello.csv', sample_size=-1);
      COPY (
          SELECT 'https://example.org/user/' || users.id AS subject,
                 'https://example.org/name' AS predicate,
                 users.name AS object
          FROM users
      ) TO 'output.parquet' (FORMAT PARQUET);
    `;
    const { rewrittenSql, vertexTables, edgeTables, tripleTables } =
      rewriteCopyToCreateTable(sql);
    expect(vertexTables).toEqual([]);
    expect(edgeTables).toEqual([]);
    expect(tripleTables).toEqual(['output']);
    expect(rewrittenSql).toMatch(/CREATE OR REPLACE TABLE "output" AS/);
    expect(rewrittenSql).toMatch(/AS subject/);
  });

  it('handles two chunks of the same vertex prefix: first chunk CREATE, second INSERT INTO', () => {
    const sql = `
      COPY ( SELECT id, name FROM src WHERE id <= 1024 ) TO 'vertex/Person/chunk0.parquet' (FORMAT PARQUET);
      COPY ( SELECT id, name FROM src WHERE id  > 1024 ) TO 'vertex/Person/chunk1.parquet' (FORMAT PARQUET);
    `;
    const { rewrittenSql, vertexTables } = rewriteCopyToCreateTable(sql);
    expect(vertexTables).toEqual(['Person']); // Single name (chunks collapse)
    expect(rewrittenSql).toMatch(/CREATE OR REPLACE TABLE "Person" AS/);
    expect(rewrittenSql).toMatch(/INSERT INTO "Person"/);
    // Exactly one CREATE; exactly one INSERT.
    expect((rewrittenSql.match(/CREATE OR REPLACE TABLE/g) ?? []).length).toBe(1);
    expect((rewrittenSql.match(/INSERT INTO/g) ?? []).length).toBe(1);
  });

  it('rewrites both a vertex AND an edge COPY into distinct tables', () => {
    const sql = `
      COPY ( SELECT iri AS id, name FROM base ) TO 'vertex/Person/chunk0.parquet' (FORMAT PARQUET);
      COPY ( SELECT iri AS src_id, friend AS dst_id FROM base ) TO 'edge/Person_knows_Person/chunk0.parquet' (FORMAT PARQUET);
    `;
    const { vertexTables, edgeTables, tripleTables } =
      rewriteCopyToCreateTable(sql);
    expect(vertexTables).toEqual(['Person']);
    expect(edgeTables).toEqual(['Person_knows_Person']);
    expect(tripleTables).toEqual([]);
  });

  it('leaves SQL without any COPYs unchanged structurally', () => {
    const sql = 'CREATE VIEW users AS SELECT * FROM something;';
    const { rewrittenSql, vertexTables, edgeTables, tripleTables } =
      rewriteCopyToCreateTable(sql);
    expect(rewrittenSql).toBe(sql);
    expect(vertexTables).toEqual([]);
    expect(edgeTables).toEqual([]);
    expect(tripleTables).toEqual([]);
  });
});

describe('transformSql (end-to-end)', () => {
  const mockResolver = (
    map: Record<string, string>,
  ): ConnectionResolver => ({
    async resolve(ref) {
      const url = map[ref.raw];
      if (!url) {
        throw new Error(`mock resolver: no fixture for ${ref.raw}`);
      }
      return { url, format: 'csv' };
    },
    async list() {
      return [];
    },
  });

  it('resolves @ref → registeredFiles entry, leaves SQL literal intact, rewrites the COPY', async () => {
    // Post-Task-3 contract: transformSql does NOT substitute the URL into
    // the SQL — DuckDB-WASM's HTTP protocol cannot resolve `blob:` URLs
    // from the Worker realm. Instead, the caller (runPipeline) fetches the
    // URL and registers the bytes under the original `@`-literal via
    // `db.registerFileBuffer`, so `read_csv_auto('@examples/hello.csv')`
    // resolves through DuckDB's virtual FS.
    const sql = `
      CREATE VIEW users AS
      SELECT * FROM read_csv_auto('@examples/hello.csv', sample_size=-1);
      COPY ( SELECT 'x' AS subject, 'y' AS predicate, 'z' AS object FROM users )
        TO 'output.parquet' (FORMAT PARQUET);
    `;
    const resolver = mockResolver({
      '@examples/hello.csv': 'blob:fake-url-abc',
    });
    // maxResolvedBytes 0 disables the cap so we don't have to stub fetch.
    const result = await transformSql(sql, resolver, { maxResolvedBytes: 0 });
    // SQL literal is PRESERVED (NOT substituted) — the original `@`-ref
    // stays so DuckDB resolves it via the virtual-FS registration the
    // caller does next.
    expect(result.executableSql).toMatch(/'@examples\/hello\.csv'/);
    expect(result.executableSql).not.toMatch(/blob:fake-url-abc/);
    expect(result.executableSql).toMatch(/CREATE OR REPLACE TABLE "output" AS/);
    expect(result.tripleTables).toEqual(['output']);
    // The resolved URL is surfaced via registeredFiles — never leaks into
    // SQL or React state. The caller uses this to drive registerFileBuffer.
    expect(result.registeredFiles).toEqual([
      { virtualName: '@examples/hello.csv', url: 'blob:fake-url-abc' },
    ]);
  });

  it('refuses oversize resolved URLs via Content-Length HEAD check (PLAY-12)', async () => {
    const sql =
      "SELECT * FROM read_csv_auto('@examples/big.csv'); COPY (SELECT 1) TO 'output.parquet' (FORMAT PARQUET);";
    const resolver = mockResolver({
      '@examples/big.csv': 'https://example.test/big.csv',
    });
    // Stub fetch to return a 50 MB Content-Length so the 10 MB cap trips.
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        headers: new Headers({ 'content-length': '50000000' }),
      }),
    );
    await expect(
      transformSql(sql, resolver, { maxResolvedBytes: 10_000_000 }),
    ).rejects.toThrow(/maxResolvedBytes cap.*50000000.*10000000/);
  });

  it('passes when Content-Length is missing (defence in depth, not a hard gate)', async () => {
    const sql =
      "SELECT * FROM read_csv_auto('@examples/sized.csv'); COPY (SELECT 1) TO 'output.parquet' (FORMAT PARQUET);";
    const resolver = mockResolver({
      '@examples/sized.csv': 'https://example.test/sized.csv',
    });
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ headers: new Headers() }),
    );
    const result = await transformSql(sql, resolver, {
      maxResolvedBytes: 1_000_000,
    });
    // Post-Task-3: URL is NOT substituted into SQL (registerFileBuffer
    // path); the original `@`-literal is preserved and the resolved URL
    // surfaces only via registeredFiles.
    expect(result.executableSql).toMatch(/'@examples\/sized\.csv'/);
    expect(result.registeredFiles).toEqual([
      { virtualName: '@examples/sized.csv', url: 'https://example.test/sized.csv' },
    ]);
  });

  it('de-duplicates identical @ref literals — one resolver.resolve call per unique literal', async () => {
    const sql = `
      CREATE VIEW a AS SELECT * FROM read_csv_auto('@examples/x.csv');
      CREATE VIEW b AS SELECT * FROM read_csv_auto('@examples/x.csv');
      COPY (SELECT 1) TO 'output.parquet' (FORMAT PARQUET);
    `;
    const resolveFn = vi.fn(async () => ({ url: 'blob:dedup-test' } as const));
    const resolver: ConnectionResolver = {
      resolve: resolveFn,
      async list() {
        return [];
      },
    };
    const result = await transformSql(sql, resolver, { maxResolvedBytes: 0 });
    expect(resolveFn).toHaveBeenCalledTimes(1);
    // Both occurrences of the original `@`-literal are PRESERVED in the SQL
    // (no substitution); single registered-file entry serves both reads via
    // DuckDB's virtual FS.
    expect(
      (result.executableSql.match(/@examples\/x\.csv/g) ?? []).length,
    ).toBe(2);
    expect(result.registeredFiles).toEqual([
      { virtualName: '@examples/x.csv', url: 'blob:dedup-test' },
    ]);
  });
});
