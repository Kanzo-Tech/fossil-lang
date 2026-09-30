/**
 * A corpus large enough to have several row groups per table, written by the guards' own fixture
 * with the `duckdb` binary and read back through `src/` on DuckDB-WASM:
 *
 * 1. **`open → manifest → scan`**: every table the manifest declares reads back as many rows as its
 *    `record_count` says.
 * 2. **A box prunes.** DuckDB-WASM's own file statistics say which bytes of the file were read, and
 *    a box in one corner of the plane reads the row groups whose footer statistics overlap it and
 *    no others — fewer than the file has. Nothing in `src/` computes that set: the predicate reaches
 *    `read_parquet` uncast and the engine does the rest.
 */

import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { DuckDBDataProtocol, type DuckDBBindings } from '@duckdb/duckdb-wasm/blocking';
import type { Engine } from '@fossil-lang/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { query as duck } from '../guards/duck.mjs';
import { ROW_GROUP_ROWS, write } from '../guards/fixture.mjs';
import { open, type Corpus, type Filter, type VertexTable } from '../src/index.js';
import { duckdb } from '../tests/engine.js';

/** Four row groups of `Person` at 122,880 rows each, the last one partial. */
const PEOPLE = 400_000;

const scratch = mkdtempSync(join(tmpdir(), 'fossil-integration-'));
const dir = join(scratch, 'corpus');
let db: DuckDBBindings;
let engine: Engine;
let corpus: Corpus;

beforeAll(async () => {
  write(dir, { count: PEOPLE, clusters: 256 });
  ({ db, engine } = await duckdb(join(scratch, 'spill')));
  corpus = await open(dir, { engine });
}, 300_000);

afterAll(async () => {
  await corpus?.close();
  rmSync(scratch, { recursive: true, force: true });
});

describe('open → manifest → scan', () => {
  it('reads every table back at its record_count', async () => {
    const tables = [...corpus.manifest.vertex_tables, ...corpus.manifest.edge_tables];
    expect(tables.length).toBe(6);
    for (const table of tables) {
      const scan = corpus.scan({ table: table.name, select: [table.properties[0]!.name] });
      const batches = await scan.read(scan.plan());
      expect(batches.reduce((n, b) => n + b.numRows, 0), table.name).toBe(table.record_count);
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

describe('a box prunes row groups', () => {
  it('reads the groups whose statistics overlap the box, and fewer than the file has', async () => {
    const person = corpus.manifest.vertex_tables.find((t) => t.name === 'Person') as VertexTable;
    const file = join(dir, person.path);
    // Lent under its own path, so the SQL the view holds reaches it and DuckDB-WASM counts its reads.
    db.registerFileURL(file, file, DuckDBDataProtocol.NODE_FS, false);
    const select = ['dense_id', 'x', 'y'];
    const [extent] = duck(`SELECT min(x) x0, max(x) x1, min(y) y0, max(y) y1 FROM '${file}'`) as Record<string, number>[];
    const { x0, x1, y0, y1 } = extent!;
    const box: [number, number, number, number] = [x0, y0, x0 + (x1 - x0) / 10, y0 + (y1 - y0) / 10];

    const total = Number(duck(`SELECT count(DISTINCT row_group_id) AS n FROM parquet_metadata('${file}')`)[0].n);
    expect(total).toBe(Math.ceil(PEOPLE / ROW_GROUP_ROWS));
    const overlapping = new Set(
      (
        duck(
          `SELECT row_group_id AS g FROM parquet_metadata('${file}')
            GROUP BY g
           HAVING max(CASE WHEN path_in_schema = 'x' THEN stats_max_value::DOUBLE END) >= ${box[0]}
              AND min(CASE WHEN path_in_schema = 'x' THEN stats_min_value::DOUBLE END) <= ${box[2]}
              AND max(CASE WHEN path_in_schema = 'y' THEN stats_max_value::DOUBLE END) >= ${box[1]}
              AND min(CASE WHEN path_in_schema = 'y' THEN stats_min_value::DOUBLE END) <= ${box[3]}`,
        ) as { g: number }[]
      ).map((r) => Number(r.g)),
    );

    const scanOf = (filter?: Filter) => corpus.scan({ table: person.name, select, ...(filter ? { filter } : {}) });
    const boxed = scanOf({ bbox: box });
    let rows = 0;
    const inBox = await groupsRead(file, select, async () => {
      rows = (await boxed.read(boxed.plan()))[0]!.numRows;
    });
    const all = scanOf();
    const everything = await groupsRead(file, select, () => all.read(all.plan()));

    expect(rows).toBeGreaterThan(0);
    expect(everything.size, 'an unfiltered scan reads every group, or the measure is blind').toBe(total);
    expect(inBox.size).toBeLessThan(total);
    expect([...inBox].sort()).toEqual([...overlapping].sort());
  }, 60_000);
});
