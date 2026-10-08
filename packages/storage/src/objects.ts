import type { Host, Problem } from '@fossil-lang/types';
import { until } from '@fossil-lang/types/internal';

import { initFossilStorage, storageRead } from './wasm.js';

/** A file to read: where it is, and the connection it lies under when it has one. */
export interface Target {
  location: string;
  connection?: string;
}

/** A target's bytes, or the problem that kept them — plain data; `FossilError.from(problem)` throws it. */
export type ReadResult = { ok: true; bytes: Uint8Array } | { ok: false; problem: Problem };

/**
 * Read each target's bytes, in order, with the credentials the host vends.
 *
 * One `credentials` call per connection named, not per file. A target with no connection is read
 * as it is only when it is a public `http(s)` URL — there is nothing to vend for it. A failure is
 * the target's own answer and never the batch's — `storage/host-silent` among them, for a host that
 * did not answer within 30 s.
 *
 * `signal` abandons the read: it rejects with the signal's reason.
 */
export async function read(
  host: Host,
  targets: readonly Target[],
  { signal }: { signal?: AbortSignal } = {},
): Promise<ReadResult[]> {
  await until(initFossilStorage(), signal);
  signal?.throwIfAborted();
  return (await until(storageRead(host, targets), signal)) as ReadResult[];
}
