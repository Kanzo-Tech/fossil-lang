import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { initFossilStorage } from '../src/index.js';

// Node's fetch rejects `file://`, so the bytes go in instead of the glue's URL.
await initFossilStorage(
  await readFile(fileURLToPath(new URL('../pkg/fossil_storage_wasm_bg.wasm', import.meta.url))),
);
