import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { initFossilExecutor } from '../src/index.js';

describe('initFossilExecutor', () => {
  it('fails a module that will not compile as internal/bug, and boots on the next call', async () => {
    await expect(initFossilExecutor(new Uint8Array([0, 1, 2, 3]))).rejects.toMatchObject({
      name: 'FossilError',
      code: 'internal/bug',
      cause: { name: 'CompileError' },
    });
    const bytes = await readFile(fileURLToPath(new URL('../pkg/fossil_df_wasm_bg.wasm', import.meta.url)));
    await expect(initFossilExecutor(bytes)).resolves.toBeDefined();
  });
});
