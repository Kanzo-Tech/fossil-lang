/**
 * runPipeline — the orchestration spine of the playground Run path.
 *
 * Closes the BLOCKER gap from 08-VERIFICATION.md (SC#1 / PLAY-01/02/03): the
 * old `handleRun` returned `{ vertices: [], edges: [] }` unconditionally; this
 * function performs the four wired steps end-to-end:
 *
 *   1. compile(mapping) → SQL  (injected: caller manages the FossilPlayground
 *      WASM instance lifecycle so this module stays test-pure)
 *   2. transformSql → resolved URLs substituted + COPYs rewritten to CREATE TABLE
 *   3. getDuckDb → lazy-boot DuckDB-WASM + register Tier-1 blob URLs as
 *      virtual files (so `read_csv_auto('@examples/foo.csv')` resolves to the
 *      blob without depending on the substituted URL surviving SQL escaping)
 *   4. execute SQL statement-by-statement on ONE connection, then SELECT
 *      every classified table back and project into VertexRow / EdgeRow
 *
 * The compile + DuckDB deps are INJECTED (not statically imported) so:
 *   - the WASM-side FossilPlayground instance lifecycle stays in the React
 *     component (where it can `free()` on unmount per ADR-0026);
 *   - vitest can mock the deps without dragging the WASM bundle in.
 */

import type { ConnectionResolver } from '@fossil-lang/types';
import type { VertexRow, EdgeRow } from '../component/FossilPlayground.js';
import { transformSql } from './transformSql.js';

/**
 * Dependencies the runPipeline reaches out to. Both injected so tests can
 * stub them without booting the real WASM or DuckDB worker.
 */
export interface RunPipelineDeps {
  /**
   * Returns the lazily-booted AsyncDuckDB instance. Production: pass
   * `getDuckDb` from `../hooks/useDuckDb.js`. Tests: pass a stub that
   * returns a mock-DB whose `connect()` returns a mock connection.
   *
   * Typed `any` to keep `@duckdb/duckdb-wasm` off the static-import graph
   * (matches the existing `useDuckDb` pattern + ADR-0025 lazy-load).
   */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  getDuckDb: () => Promise<any>;

  /**
   * Compile a `.fossil` mapping source → DuckDB SQL string. Callers wrap
   * the WASM `FossilPlayground.compileFile(handle)` API; this module never
   * touches the WASM instance directly. Implementations typically memoise
   * an `openFile` handle and `updateFile` on subsequent calls so Salsa's
   * incremental memoisation kicks in (ADR-0022).
   */
  compile: (mapping: string) => Promise<string>;
}

/** Input the runPipeline operates on. */
export interface RunPipelineInput {
  /** Host-injected resolver (Tier-1 default or Tier-2 host-mediated). */
  resolver: ConnectionResolver;
  /** Current `.fossil` mapping text from the editor. */
  mapping: string;
  /**
   * Content-Length cap (bytes). The HEAD-check refuses any URL whose
   * Content-Length header exceeds this value BEFORE bytes reach DuckDB
   * (PLAY-12). `0` or negative disables the cap entirely.
   */
  maxResolvedBytes: number;
}

/** Vertex+edge rows the runPipeline returns + the React component renders. */
export interface RunPipelineResult {
  vertices: VertexRow[];
  edges: EdgeRow[];
}

/**
 * Find a column value by trying a list of preferred names then falling back
 * to the first column. Defensive — different descriptors emit different
 * column names (`iri` vs `id`, `src_id` vs `subject`).
 */
function pickColumn(
  row: Record<string, unknown>,
  candidates: readonly string[],
  fallback: string,
): unknown {
  for (const name of candidates) {
    if (name in row) return row[name];
  }
  // First column (insertion order — V8 + modern JS engines guarantee).
  const firstKey = Object.keys(row)[0];
  if (firstKey !== undefined) return row[firstKey];
  return fallback;
}

/**
 * Project a vertex table row → VertexRow. The canonical column name is `id`
 * (the GraphAr vertex-table convention from `vertex_select_sql` which
 * `SELECT iri AS id ...`); for the AcceptAll path we also accept the raw
 * `iri` column. Extra columns flow through as properties on the row.
 */
function projectVertex(row: Record<string, unknown>): VertexRow {
  const id = String(pickColumn(row, ['id', 'iri', 'subject'], ''));
  return { ...row, id };
}

/**
 * Project an edge table row → EdgeRow. GraphAr edge tables use `src_id` /
 * `dst_id` (per `edge_select_sql`); AcceptAll flat triples use `subject` /
 * `object`. Either way we surface a stable `source` / `target` shape so
 * `ResultTable` renders consistent column headers.
 */
function projectEdge(row: Record<string, unknown>): EdgeRow {
  const source = String(pickColumn(row, ['src_id', 'source', 'subject'], ''));
  const target = String(pickColumn(row, ['dst_id', 'target', 'object'], ''));
  return { ...row, source, target };
}

/**
 * Split a SQL script into individual statements at top-level `;` boundaries.
 * The codegen-emitted SQL uses `;` only at statement terminators (string
 * literals don't contain bare `;` — IRIs use `;` only after `://`, which is
 * not preceded by a quote). The simple split is sufficient for v0.1; if the
 * codegen ever emits string literals with embedded `;`, this helper will
 * need a tokenising splitter.
 *
 * Trailing/leading whitespace is trimmed; empty statements are dropped.
 */
function splitSqlStatements(sql: string): string[] {
  return sql
    .split(';')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Materialise an Arrow Table into plain JS rows. `.toArray()` returns an
 * array of row objects with column-name keys (apache-arrow's documented
 * shape); BigInts and Dates flow through as-is — `ResultTable.renderCell`
 * handles the per-type coercion when rendering.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function tableToRows(table: any): Record<string, unknown>[] {
  // apache-arrow's Table.toArray() returns Row[]; each Row is plain JSON.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (table?.toArray?.() ?? []) as Record<string, unknown>[];
}

/**
 * Fetch a resolver-returned URL and return its bytes. Used for the
 * `registerFileBuffer` virtual-FS registration path (see comment block on
 * `transformSql` for why we don't use DuckDB's HTTP protocol for blob URLs).
 *
 * Main-thread fetch — works for both `blob:` and `https:` URLs uniformly.
 * Errors propagate to the caller so the SQL execution doesn't proceed against
 * a missing virtual file.
 */
async function fetchUrlBytes(url: string): Promise<Uint8Array> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(
      `runPipeline: failed to fetch resolved source (${response.status} ${response.statusText}): ${url}`,
    );
  }
  const buffer = await response.arrayBuffer();
  return new Uint8Array(buffer);
}

/**
 * The Run pipeline. Orchestrates compile → transformSql → DuckDB execute →
 * vertex/edge projection. Throws if any step fails; callers (React
 * component) surface the error through `runError` state + `onError`.
 *
 * Connection lifecycle: one `db.connect()` per Run call, closed in the
 * `finally` block so errors don't leak connections across Runs. The DuckDB
 * Worker itself is module-singleton (see `useDuckDb.getDuckDb`) — recreated
 * only on Reset per ADR-0026.
 */
export async function runPipeline(
  deps: RunPipelineDeps,
  input: RunPipelineInput,
): Promise<RunPipelineResult> {
  const { resolver, mapping, maxResolvedBytes } = input;

  // Step 1: compile mapping → SQL via the injected compile fn.
  const sql = await deps.compile(mapping);
  if (!sql) {
    // Empty SQL (the test-stub path) is a successful no-op — return empty
    // result rather than fail. Production compile never returns empty.
    return { vertices: [], edges: [] };
  }

  // Step 2: resolve refs + substitute URLs + rewrite COPYs.
  const transformed = await transformSql(sql, resolver, { maxResolvedBytes });

  // Step 3: lazy-boot DuckDB-WASM + register every resolved source as a
  // virtual file under its `@connector/path` name, so the SQL's untouched
  // `read_csv_auto('@examples/hello.csv')` reads through DuckDB's in-memory
  // FS instead of going out over the network.
  //
  // Why fetch-then-registerFileBuffer (not registerFileURL with HTTP
  // protocol): DuckDB-WASM's HTTP protocol runs inside the DuckDB Worker,
  // which cannot resolve `blob:` URLs created on the main thread (the HEAD
  // pre-flight fails with `net::ERR_METHOD_NOT_SUPPORTED`). Fetching on
  // the main thread + buffering the bytes works uniformly for blob/https/
  // anything-fetch-can-reach. See `transformSql.ts` for the longer write-up.
  const db = await deps.getDuckDb();
  for (const { virtualName, url } of transformed.registeredFiles) {
    const bytes = await fetchUrlBytes(url);
    await db.registerFileBuffer(virtualName, bytes);
  }

  // Step 4: execute on ONE connection then read back.
  //
  // BUG-01 fix (Phase 15 plan 15-01): the DuckDB-WASM Worker is module-
  // singleton (per ADR-0026 — recreated only on Reset). Views + tables
  // created during the first Run persist into the second Run on the same
  // worker. The Rust codegen emits plain `CREATE VIEW` / `CREATE TABLE`
  // (not `CREATE OR REPLACE`) — which DuckDB rejects on re-execution
  // with `Catalog Error: View with name "<name>" already exists`.
  //
  // We can't change the Rust codegen (per 15-CONTEXT.md "NO toca compiler
  // Rust"), so the playground patches the executable SQL stream on the
  // way to the DuckDB Worker: rewrite each `CREATE VIEW <name>` /
  // `CREATE TABLE <name>` into the idempotent `CREATE OR REPLACE …`
  // form. The rewriter is intentionally narrow — it only matches
  // statement-prefix `CREATE (VIEW|TABLE)` (no `OR REPLACE` already
  // present, no `TEMP`/`TEMPORARY` qualifier), which is the exact shape
  // codegen-emits in v0.2. If codegen ever grows a `TEMP VIEW` path, the
  // regex will need an additional alternation.
  //
  // `rewriteCopyToCreateTable` (called inside transformSql) already emits
  // `CREATE OR REPLACE TABLE` for the COPY→TABLE rewrite, so the second
  // Run's vertex/edge/output tables are safe. Only the upstream `CREATE
  // VIEW <source>` statements need this fix.
  const idempotentSql = transformed.executableSql.replace(
    /\bCREATE\s+(VIEW|TABLE)\b(?!\s+OR\s+REPLACE)/gi,
    'CREATE OR REPLACE $1',
  );

  const conn = await db.connect();
  try {
    const statements = splitSqlStatements(idempotentSql);
    for (const stmt of statements) {
      await conn.query(stmt);
    }

    const vertices: VertexRow[] = [];
    const edges: EdgeRow[] = [];

    for (const name of transformed.vertexTables) {
      const table = await conn.query(`SELECT * FROM "${name}"`);
      for (const row of tableToRows(table)) {
        vertices.push(projectVertex(row));
      }
    }
    for (const name of transformed.edgeTables) {
      const table = await conn.query(`SELECT * FROM "${name}"`);
      for (const row of tableToRows(table)) {
        edges.push(projectEdge(row));
      }
    }
    for (const name of transformed.tripleTables) {
      // AcceptAll path: flat (subject, predicate, object) rows. Project as
      // BOTH vertices (distinct subjects) and edges (every triple becomes
      // one degenerate row) so the playground always renders something.
      const vTable = await conn.query(
        `SELECT DISTINCT subject FROM "${name}"`,
      );
      for (const row of tableToRows(vTable)) {
        vertices.push(projectVertex(row));
      }
      const eTable = await conn.query(
        `SELECT subject, predicate, object FROM "${name}"`,
      );
      for (const row of tableToRows(eTable)) {
        edges.push(projectEdge(row));
      }
    }

    return { vertices, edges };
  } finally {
    try {
      await conn.close();
    } catch {
      // Closing a borked connection sometimes throws — swallow to surface
      // the original query error to the caller.
    }
  }
}
