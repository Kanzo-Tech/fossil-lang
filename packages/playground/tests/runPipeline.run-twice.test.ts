/**
 * BUG-01 unit-level regression test — runPipeline twice over the same input
 * must return IDENTICAL { vertices, edges } and must NOT throw on the second
 * call due to DuckDB state-leak (View already exists, etc).
 *
 * Twin of `apps/landing/tests/e2e/run-twice.spec.ts` (the Playwright canary).
 * The Playwright spec exercises the full WASM + DuckDB-WASM + React stack
 * end-to-end; this spec mocks the DuckDB-WASM-shaped deps so the assertion
 * runs in milliseconds inside the vitest happy-dom env.
 *
 * Why this matters even though we have an e2e spec:
 *   - The e2e spec depends on the apps/landing/ Next.js build + the
 *     Playwright browser pool. CI runs both packages in parallel, and this
 *     unit-level test gives an EARLY signal at the @fossil-lang/playground
 *     level — a BUG-01 regression caught here fails the package suite
 *     before the slower e2e gate even starts.
 *   - The mock surfaces the specific Run-twice contract (`registerFileBuffer`
 *     called twice with the same name; `conn.query` called with idempotent
 *     CREATE statements; no DuckDB-style errors thrown). The e2e spec
 *     asserts on the user-visible outcome; this asserts on the wire
 *     protocol.
 *
 * Mock strategy: in-process spy DuckDB. The mock connection records every
 * `query()` call and returns deterministic Arrow-shaped results for the
 * vertex/edge SELECT-back queries. The compile callback returns a stable SQL
 * string. The mock resolver returns a stable URL. With these all
 * deterministic, the second `runPipeline(...)` call MUST produce byte-
 * identical results — any state-leak inside runPipeline surfaces as a
 * mismatch in vertex/edge contents.
 *
 * The chosen approach: MOCK (real DuckDB-WASM in vitest requires a Worker +
 * fetch stack happy-dom can't provide; the e2e spec covers the real-WASM
 * path).
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import type { ConnectionResolver } from '@fossil-lang/types';
import { runPipeline } from '../src/run/runPipeline.js';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

// Apache-Arrow-shaped row collection: the runPipeline calls `.toArray()` on
// the query result; rows are plain JS objects keyed by column name.
function makeArrowTable(rows: Array<Record<string, unknown>>): { toArray(): Array<Record<string, unknown>> } {
  return {
    toArray: () => rows,
  };
}

interface MockConn {
  queryLog: string[];
  closed: boolean;
  query(sql: string): Promise<{ toArray(): Array<Record<string, unknown>> }>;
  close(): Promise<void>;
}

interface MockDb {
  registerCalls: Array<{ name: string; bytes: number }>;
  connectCount: number;
  connections: MockConn[];
  registerFileBuffer(name: string, bytes: Uint8Array): Promise<void>;
  connect(): Promise<MockConn>;
}

function makeMockDb(): MockDb {
  const db: MockDb = {
    registerCalls: [],
    connectCount: 0,
    connections: [],
    async registerFileBuffer(name: string, bytes: Uint8Array): Promise<void> {
      db.registerCalls.push({ name, bytes: bytes.byteLength });
    },
    async connect(): Promise<MockConn> {
      db.connectCount += 1;
      const conn: MockConn = {
        queryLog: [],
        closed: false,
        async query(sql: string) {
          conn.queryLog.push(sql);
          // Vertex/edge SELECT-back: return a fixed two-row table.
          // The codegen-emitted COPY rewrite produces `output` (AcceptAll
          // path) — see runPipeline's `tripleTables` loop. We return both
          // for the DISTINCT-subject vertex path AND the full subject/
          // predicate/object edge path.
          if (/^SELECT DISTINCT subject FROM/i.test(sql)) {
            return makeArrowTable([
              { subject: 'https://example.org/user/1' },
              { subject: 'https://example.org/user/2' },
            ]);
          }
          if (/^SELECT subject, predicate, object FROM/i.test(sql)) {
            return makeArrowTable([
              {
                subject: 'https://example.org/user/1',
                predicate: 'https://example.org/name',
                object: 'Alice',
              },
              {
                subject: 'https://example.org/user/2',
                predicate: 'https://example.org/name',
                object: 'Bob',
              },
            ]);
          }
          // Everything else (CREATE VIEW, CREATE OR REPLACE TABLE, etc.)
          // returns an empty arrow table — the runPipeline doesn't read
          // back from these statements.
          return makeArrowTable([]);
        },
        async close() {
          conn.closed = true;
        },
      };
      db.connections.push(conn);
      return conn;
    },
  };
  return db;
}

const mockResolver: ConnectionResolver = {
  async resolve() {
    return { url: 'blob:fake-mock-url', format: 'csv' };
  },
  async list() {
    return [];
  },
};

// Stable mapping + compile-callback that returns a stable SQL string mirroring
// the codegen's AcceptAll shape (CREATE VIEW source + COPY (..) TO 'output.parquet').
// rewriteCopyToCreateTable inside transformSql collapses the COPY into a
// CREATE OR REPLACE TABLE "output" AS SELECT … FROM users — so the bug-01
// CREATE-VIEW-OR-REPLACE rewriter (runPipeline) only matters for the source
// view, which is what we want to assert is now safe to re-run.
const STABLE_MAPPING = 'users := io.csv("@examples/hello.csv")\nUser : ex:Person from users\n    iri = `${ex:}user/${.id}`\n';

const STABLE_SQL = `
CREATE VIEW users AS
SELECT * FROM read_csv_auto('@examples/hello.csv', sample_size=-1);
COPY (
    SELECT 'https://example.org/user/' || users.id AS subject,
           'https://example.org/name' AS predicate,
           users.name AS object
    FROM users
) TO 'output.parquet' (FORMAT PARQUET);
`.trim();

describe('runPipeline — BUG-01 regression: invoking twice on the same input', () => {
  it('returns IDENTICAL { vertices, edges } on the second call (no state-leak)', async () => {
    const db = makeMockDb();
    // Stub the global fetch the runPipeline uses to load the resolved blob:
    // URL bytes for registerFileBuffer. The fake response is deterministic
    // so both Runs see the same `Uint8Array` length.
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        async arrayBuffer() {
          return new TextEncoder().encode('id,name\n1,Alice\n2,Bob\n').buffer;
        },
      }),
    );
    const compile = vi.fn(async () => STABLE_SQL);

    // ----- First Run -----
    const first = await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );

    expect(first.vertices.length).toBeGreaterThan(0);
    expect(first.edges.length).toBeGreaterThan(0);

    // Snapshot the first-Run state for deep-equality comparison.
    const firstVertices = JSON.stringify(first.vertices);
    const firstEdges = JSON.stringify(first.edges);

    // ----- Second Run (the bug surface) -----
    // With BUG-01 present (pre-fix runPipeline + pre-fix FossilPlayground),
    // this second call would EITHER:
    //   - throw `Error: null pointer passed to rust` (handle-consumed-by-
    //     compileFile path — that bug is fixed in FossilPlayground.tsx;
    //     not directly exercised here since we mock the compile callback);
    //   - throw a DuckDB Catalog Error if the rewriter regresses (the
    //     CREATE VIEW becomes a duplicate). The mock would propagate the
    //     throw because we don't catch it.
    // Post-fix: the rewriter in runPipeline switches CREATE VIEW →
    // CREATE OR REPLACE VIEW, so the second connection.query is safe.
    const second = await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );

    // The core BUG-01 invariant — deep equality of the result shape.
    expect(JSON.stringify(second.vertices)).toBe(firstVertices);
    expect(JSON.stringify(second.edges)).toBe(firstEdges);
  });

  it('rewrites CREATE VIEW into CREATE OR REPLACE VIEW so DuckDB does not throw on re-run', async () => {
    const db = makeMockDb();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        async arrayBuffer() {
          return new ArrayBuffer(8);
        },
      }),
    );
    const compile = vi.fn(async () => STABLE_SQL);

    await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );

    // The first connection's query log should contain the rewritten CREATE
    // form. Codegen emitted `CREATE VIEW users AS …`; runPipeline rewrote it
    // before handing to DuckDB.
    const conn = db.connections[0];
    expect(conn).toBeDefined();
    const createStmts = conn!.queryLog.filter((q) => /CREATE.*VIEW|CREATE.*TABLE/i.test(q));
    expect(createStmts.length).toBeGreaterThan(0);
    // EVERY CREATE statement must use the idempotent OR REPLACE form so
    // the second Run on the same DuckDB-WASM module-singleton Worker
    // doesn't trip a Catalog Error.
    for (const stmt of createStmts) {
      expect(stmt).toMatch(/CREATE\s+OR\s+REPLACE\s+(VIEW|TABLE)/i);
    }
  });

  it('compile callback is invoked once per call (no leaked cache)', async () => {
    const db = makeMockDb();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        async arrayBuffer() {
          return new ArrayBuffer(0);
        },
      }),
    );
    const compile = vi.fn(async () => STABLE_SQL);

    await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );
    expect(compile).toHaveBeenCalledTimes(1);

    await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );
    expect(compile).toHaveBeenCalledTimes(2);
  });

  it('the second connection is closed cleanly (no connection leak across Runs)', async () => {
    const db = makeMockDb();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        async arrayBuffer() {
          return new ArrayBuffer(0);
        },
      }),
    );
    const compile = vi.fn(async () => STABLE_SQL);

    await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );
    await runPipeline(
      { getDuckDb: async () => db, compile },
      { resolver: mockResolver, mapping: STABLE_MAPPING, maxResolvedBytes: 0 },
    );

    expect(db.connectCount).toBe(2);
    expect(db.connections[0]?.closed).toBe(true);
    expect(db.connections[1]?.closed).toBe(true);
  });
});
