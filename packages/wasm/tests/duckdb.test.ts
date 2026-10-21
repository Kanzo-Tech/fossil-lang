/**
 * `duckdbTypeOf` across the wasm boundary. The table is the Rust's (`fossil_graph_schema::Duckdb`),
 * held against a graph DuckDB read back in `fossil-exec`'s `declared_type_reaches_the_column`;
 * this suite holds that the export reaches JS, answers every primitive and refuses what is not one.
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import type { Primitive } from '@fossil-lang/types';
import { duckdbTypeOf, initFossilWasm } from '../src/index.js';

beforeAll(async () => {
  await initFossilWasm(await readFile(fileURLToPath(new URL('../pkg/fossil_wasm_bg.wasm', import.meta.url))));
});

describe('@fossil-lang/wasm — duckdbTypeOf', () => {
  it('names the type a written column of each primitive has', async () => {
    const every: Record<Primitive, string> = {
      string: 'VARCHAR',
      string_lang: 'STRUCT("value" VARCHAR, lang VARCHAR)',
      integer: 'BIGINT',
      float: 'DOUBLE',
      bool: 'BOOLEAN',
      time_year: 'INTEGER',
      time_date: 'DATE',
      time_date_time: 'TIMESTAMP',
      time_hour: 'VARCHAR',
      uri: 'VARCHAR',
    };
    for (const [primitive, type] of Object.entries(every)) {
      expect(await duckdbTypeOf(primitive as Primitive), primitive).toBe(type);
    }
  });

  it('refuses what is not a primitive', async () => {
    await expect(duckdbTypeOf('date' as Primitive)).rejects.toMatchObject({ code: 'api/invalid-argument' });
  });
});
