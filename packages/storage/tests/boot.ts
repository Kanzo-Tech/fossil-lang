import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { initStorage } from '../src/index.js';

// Node's fetch rejects `file://`, so the bytes go in instead of the glue's URL.
await initStorage(
  await readFile(fileURLToPath(new URL('../pkg/fossil_storage_wasm_bg.wasm', import.meta.url))),
);
