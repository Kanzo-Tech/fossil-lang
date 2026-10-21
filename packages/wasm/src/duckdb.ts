/**
 * The DuckDB type a column of a primitive has in a graph fossil wrote, before anything has run:
 * `BIGINT` for an `integer`, `TIMESTAMP` for a `time_date_time`. The writer stores each
 * primitive as one GraphAr type whatever the sources held, so this is what a reader of the graph
 * will `DESCRIBE`. The table is `fossil_graph_schema::Duckdb`'s, in the module.
 */
import type { Primitive } from '@fossil-lang/types';
import { duckdbTypeOf as raw } from '../pkg/fossil_wasm.js';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** The DuckDB type of a written `primitive` column. Boots the module itself; `signal` stops the wait for the boot. */
export async function duckdbTypeOf(
  primitive: Primitive,
  { signal }: { signal?: AbortSignal } = {},
): Promise<string> {
  await until(initFossilWasm(), signal);
  return raw(primitive);
}
