import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

import { initStorage } from '@fossil-lang/storage';

/**
 * `@fossil-lang/storage`'s WASM, for the one test that opens a job's corpus through a host.
 *
 * `--target web` init() defaults to `fetch(url)`, and Node's fetch rejects `file://` — so read the
 * bytes and pass a BufferSource.
 */
await initStorage(
  await readFile(createRequire(import.meta.url).resolve('@fossil-lang/storage/pkg/fossil_storage_wasm_bg.wasm')),
);
