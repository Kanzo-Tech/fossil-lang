import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { initFossilGraphWasm } from '../src/load.js';

/**
 * The WASM module, instantiated before the file that imported this is collected.
 *
 * **Every test that opens or addresses a corpus needs it now.** `resolveCorpus` used to be
 * synchronous arithmetic in TypeScript over a parsed manifest, so a test could build one and
 * compose URLs with nothing loaded; it asks `fossil-graph` through `fossil-graph-wasm` instead, and
 * that module has to be up first. This is a module and not a `beforeAll` because two of the callers
 * resolve a corpus at collection time, to generate a test per row of a published table.
 *
 * `--target web` init() defaults to `fetch(url)`, and Node's fetch rejects `file://` — so read the
 * bytes and pass a BufferSource, the same way `@fossil-lang/wasm`'s tests do.
 */
await initFossilGraphWasm({
  wasmUrl: (await readFile(
    fileURLToPath(new URL('../pkg/fossil_graph_wasm_bg.wasm', import.meta.url)),
  )) as unknown as URL,
});
