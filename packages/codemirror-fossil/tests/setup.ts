/**
 * Test setup — boot the @fossil-lang/wasm module once for all tests in this
 * package. Mirrors `packages/wasm/tests/tokenize.test.ts`'s init pattern:
 * we read the bytes via node:fs (rather than fetching `file://` URLs, which
 * is environment-dependent across Node versions) and pass them through the
 * test-internal `BufferSource` cast.
 */
import { beforeAll } from 'vitest';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import { initFossilWasm } from '@fossil-lang/wasm';

beforeAll(async () => {
  const wasmPath = fileURLToPath(
    new URL('../../wasm/pkg/fossil_wasm_bg.wasm', import.meta.url),
  );
  const bytes = await readFile(wasmPath);
  // BufferSource accepted by wasm-bindgen --target web init() at runtime;
  // the public `InitFossilWasmOpts` type narrows to URL/string/Request/
  // Response, so we cast in this test-internal harness only. Matches the
  // exact pattern in packages/wasm/tests/tokenize.test.ts.
  await initFossilWasm({ wasmUrl: bytes as unknown as URL });
});
