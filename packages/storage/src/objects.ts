import type { Host } from '@fossil-lang/types';

import { initStorage, storageRead } from './wasm.js';

/** A file to read: where it is, and the connection it lies under when it has one. */
export interface Target {
  locator: string;
  connection?: string;
}

export type ReadResult = { ok: true; bytes: Uint8Array } | { ok: false; reason: string };

/**
 * Read each target's bytes, in order, with the credentials the host vends.
 *
 * One `credentials` call per connection named, not per file. A target with no connection is read
 * as it is only when it is a public `http(s)` URL — there is nothing to vend for it. A failure is
 * the target's own answer and never the batch's.
 */
export async function read(host: Host, targets: readonly Target[]): Promise<ReadResult[]> {
  await initStorage();
  return (await storageRead(host, targets)) as ReadResult[];
}
