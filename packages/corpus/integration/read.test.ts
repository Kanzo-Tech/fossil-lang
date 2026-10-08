/**
 * A corpus large enough to have several row groups per table, written by the guards' own fixture
 * with the `duckdb` binary and read back through `src/` on DuckDB-WASM:
 *
 * 1. **`attach`, then SQL**: every table `fossil_tables` lists reads back as many rows as it says.
 * 2. **A `dense_id` range prunes.** DuckDB-WASM's own file statistics say which bytes of the file
 *    were read, and a range of ids reads the row groups whose footer statistics overlap it and no
 *    others — fewer than the file has. Nothing in `src/` computes that set: the view is
 *    `read_parquet` and the engine does the rest.
 */

import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { DuckDBDataProtocol, type DuckDBBindings } from '@duckdb/duckdb-wasm/blocking';
import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { query as duck } from '../guards/duck.mjs';
import { ROW_GROUP_ROWS, write } from '../guards/fixture.mjs';
import { attach, type Attachment } from '../src/index.js';
import type { Manifest } from '../src/manifest.js';
import { duckdb } from '../tests/engine.js';

/** Four row groups of `Person` at 122,880 rows each, the last one partial. */
const PEOPLE = 400_000;

const scratch = mkdtempSync(join(tmpdir(), 'fossil-integration-'));
const dir = join(scratch, 'corpus');
let db: DuckDBBindings;
let engine: Engine;
let corpus: Attachment | undefined;
let rows: (sql: string) => Promise<Record<string, unknown>[]>;

beforeAll(async () => {
  write(dir, { count: PEOPLE });
  ({ db, engine, query: rows } = await duckdb(join(scratch, 'spill')));
  corpus = await attach('big', { engine, url: dir });
}, 300_000);

afterAll(async () => {
  await corpus?.detach();
  rmSync(scratch, { recursive: true, force: true });
});

describe('attach, then SQL', () => {
  it('reads every table back at its record_count', async () => {
    const tables = await rows('SELECT table_name AS t, record_count::BIGINT AS n FROM big.fossil_tables');
    const manifest = JSON.parse(readFileSync(join(dir, 'fossil.json'), 'utf8')) as Manifest;
    expect(tables.map((r) => r.t)).toEqual(
      [...manifest.vertex_tables, ...manifest.edge_tables, ...(manifest.property_tables ?? [])].map((t) => t.name),
    );
    for (const { t, n } of tables) {
      const [read] = await rows(`SELECT count(*)::BIGINT AS n FROM big."${String(t)}"`);
      expect(read!.n, String(t)).toBe(n);
    }
  }, 60_000);
});

interface Span {
  readonly group: number;
  readonly start: number;
  readonly end: number;
}

/** The byte span of each column chunk of `columns`, per row group, off the footer. */
function spans(file: string, columns: readonly string[]): Span[] {
  const rows = duck(
    `SELECT row_group_id AS g, coalesce(dictionary_page_offset, data_page_offset) AS s, total_compressed_size AS n
       FROM parquet_metadata('${file}') WHERE path_in_schema IN (${columns.map((c) => `'${c}'`).join(', ')})`,
  ) as { g: number; s: number; n: number }[];
  return rows.map((r) => ({ group: Number(r.g), start: Number(r.s), end: Number(r.s) + Number(r.n) }));
}

/**
 * Which row groups a read touched: a group counts when a block read cold lies inside one of its
 * column chunks and inside no other group's. Blocks straddling two groups decide nothing.
 */
function groupsRead(file: string, columns: readonly string[], read: () => Promise<unknown>): Promise<Set<number>> {
  return (async () => {
    db.collectFileStatistics(file, true);
    await read();
    const stats = db.exportFileStatistics(file);
    db.collectFileStatistics(file, false);
    const chunks = spans(file, columns);
    const touched = new Set<number>();
    const blocks = Math.ceil(Math.max(...chunks.map((c) => c.end)) / stats.blockSize);
    for (let b = 0; b < blocks; b += 1) {
      if (stats.getBlockStats(b).file_reads_cold === 0) continue;
      const [lo, hi] = [b * stats.blockSize, (b + 1) * stats.blockSize];
      const owners = new Set(chunks.filter((c) => c.start < hi && c.end > lo).map((c) => c.group));
      if (owners.size === 1) touched.add([...owners][0]!);
    }
    return touched;
  })();
}

describe('a dense_id range prunes row groups', () => {
  it('reads the groups whose statistics overlap the range, and fewer than the file has', async () => {
    const file = join(dir, 'vertex', 'Person.parquet');
    // Lent under its own path, so the SQL the view holds reaches it and DuckDB-WASM counts its reads.
    db.registerFileURL(file, file, DuckDBDataProtocol.NODE_FS, false);
    const select = ['dense_id', 'subject'];
    const [lo, hi] = [1_000, 1_000 + Math.floor(PEOPLE / 10)];

    const total = Number(duck(`SELECT count(DISTINCT row_group_id) AS n FROM parquet_metadata('${file}')`)[0].n);
    expect(total).toBe(Math.ceil(PEOPLE / ROW_GROUP_ROWS));
    const overlapping = new Set(
      (
        duck(
          `SELECT row_group_id AS g FROM parquet_metadata('${file}') WHERE path_in_schema = 'dense_id'
              AND stats_max_value::BIGINT >= ${lo} AND stats_min_value::BIGINT <= ${hi}`,
        ) as { g: number }[]
      ).map((r) => Number(r.g)),
    );

    const read = (where: string) => engine.query(`SELECT ${select.join(', ')} FROM big."Person" ${where}`, { signal: new AbortController().signal });
    let n = 0;
    const inRange = await groupsRead(file, select, async () => {
      n = (await read(`WHERE dense_id BETWEEN ${lo} AND ${hi}`)).numRows;
    });
    const everything = await groupsRead(file, select, () => read(''));

    expect(n).toBe(hi - lo + 1);
    expect(everything.size, 'an unfiltered read touches every group, or the measure is blind').toBe(total);
    expect(inRange.size).toBeLessThan(total);
    expect([...inRange].sort()).toEqual([...overlapping].sort());
  }, 60_000);
});
