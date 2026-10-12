/**
 * `fossil-profile`'s plan, run on the host's engine: the wasm `Profiler` hands out each phase's
 * queries, the host's `query` answers them, and the profile is what the answers say. The plan is
 * Rust; this is only the loop.
 */
import { Profiler } from '../pkg/fossil_wasm.js';
import type { Profile, ProfileOptions, ProfileSource } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** One query's answer: its rows, every cell the text of a value or `null`. */
export type ProfileRows = (string | null)[][];

/**
 * Profile `sources`: each query the plan hands out is run by `query`, the queries of one phase at
 * once. A query `query` rejects is a failure in the profile, and the plan goes on; an abort of
 * `signal` rejects with its reason. Boots the module itself.
 */
export async function runProfiler(
  sources: readonly ProfileSource[],
  query: (sql: string) => Promise<ProfileRows>,
  { options, signal }: { options?: ProfileOptions; signal?: AbortSignal } = {},
): Promise<Profile> {
  await until(initFossilWasm(), signal);
  const plan = new Profiler([...sources], options);
  try {
    for (;;) {
      const phase = [];
      for (let next = plan.nextQuery(); next !== undefined; next = plan.nextQuery()) phase.push(next);
      if (phase.length === 0) return plan.finish();
      await Promise.all(
        phase.map(async ({ id, sql }) => {
          let rows: ProfileRows;
          try {
            rows = await query(sql);
          } catch (error) {
            signal?.throwIfAborted();
            plan.fail(id, error instanceof Error ? error.message : String(error));
            return;
          }
          plan.feed(id, rows);
        }),
      );
    }
  } finally {
    plan.free();
  }
}
