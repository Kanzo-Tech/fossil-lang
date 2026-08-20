import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { afterAll, describe, expect, it } from 'vitest';

// Read from the module this process already has, so the child's answer is compared against a second
// load of the same source rather than against a transcribed constant.
import { GRAPH_INFO_PATH } from '../src/address.js';

/**
 * The addressing half opens a corpus with no WASM at all — and this is the test that says so.
 *
 * It was a sentence in `index.ts` and it was false. `exports` published one entry, `.`, whose
 * module graph reaches `client.js` and `load.js`, and both of those static-import
 * `../pkg/fossil_graph_wasm.js` — a gitignored wasm-bindgen output. A consumer without the Rust
 * toolchain could not load the package at all, so "synchronous, no WASM, a notebook can use it"
 * described a module nobody outside this repo could reach. Every test in this directory imports
 * `../src/index.js`, so nothing noticed.
 *
 * The proof has to be a resolution, not an import in this process: vitest resolves `../src/*.ts`
 * off the filesystem and would pass whatever `exports` said. So:
 *
 *   1. compile `src/address.ts` and whatever it pulls in — nothing but `manifest.ts` — into a
 *      package directory built from scratch, holding the real `package.json` and **no `pkg/`**;
 *   2. link that directory into a consumer's `node_modules` and import
 *      `@fossil-lang/graph/address` from a child Node process.
 *
 * If the closure ever reaches the verb half, `tsc` emits `client.js` beside it, its
 * `import '../pkg/fossil_graph_wasm.js'` resolves to nothing, and the child dies with
 * `ERR_MODULE_NOT_FOUND` naming the file. If the subpath is dropped from `exports`, it dies with
 * `ERR_PACKAGE_PATH_NOT_EXPORTED`. Both are the defect, and both are red here.
 */

const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const manifest = JSON.parse(readFileSync(join(packageRoot, 'package.json'), 'utf8')) as {
  exports: Record<string, { types: string; import: string } | string>;
};

const work = mkdtempSync(join(tmpdir(), 'fossil-graph-address-'));
afterAll(() => rmSync(work, { recursive: true, force: true }));

describe('@fossil-lang/graph/address — reachable without the WASM toolchain', () => {
  it('publishes the subpath the addressing half is reached by', () => {
    expect(manifest.exports['./address']).toEqual({
      types: './dist/address.d.ts',
      import: './dist/address.js',
    });
  });

  it('imports from a package directory that has no pkg/ in it', () => {
    const linked = join(work, 'graph');
    mkdirSync(join(linked, 'dist'), { recursive: true });

    // Only `src/address.ts` is a root. tsc follows its imports, so whatever the closure really is
    // is what lands in `dist/` — this step is the measurement, not a copy of a list.
    execFileSync(
      join(packageRoot, 'node_modules', '.bin', 'tsc'),
      [
        'src/address.ts',
        '--outDir',
        join(linked, 'dist'),
        '--module',
        'nodenext',
        '--moduleResolution',
        'nodenext',
        '--target',
        'es2022',
        '--skipLibCheck',
        '--declaration',
        'false',
      ],
      { cwd: packageRoot, stdio: 'pipe' },
    );

    writeFileSync(join(linked, 'package.json'), readFileSync(join(packageRoot, 'package.json')));
    expect(existsSync(join(linked, 'pkg'))).toBe(false);
    expect(existsSync(join(linked, 'dist', 'client.js'))).toBe(false);
    expect(existsSync(join(linked, 'dist', 'load.js'))).toBe(false);

    const consumer = join(work, 'consumer');
    mkdirSync(join(consumer, 'node_modules', '@fossil-lang'), { recursive: true });
    symlinkSync(linked, join(consumer, 'node_modules', '@fossil-lang', 'graph'), 'dir');
    writeFileSync(join(consumer, 'package.json'), '{ "type": "module" }');
    writeFileSync(
      join(consumer, 'probe.mjs'),
      [
        "import { CorpusManifestError, GRAPH_INFO_PATH, resolveCorpus, shiftFor, tileOf, TILE_SHIFT } from '@fossil-lang/graph/address';",
        'process.stdout.write(JSON.stringify({',
        '  tile: String(tileOf(4096n)),',
        '  shift: String(TILE_SHIFT),',
        '  info: GRAPH_INFO_PATH,',
        '  kinds: [typeof resolveCorpus, typeof shiftFor, typeof CorpusManifestError],',
        '}));',
      ].join('\n'),
    );

    const stdout = execFileSync(process.execPath, ['probe.mjs'], { cwd: consumer, encoding: 'utf8' });

    expect(JSON.parse(stdout)).toEqual({
      tile: '1',
      shift: '12',
      info: GRAPH_INFO_PATH,
      kinds: ['function', 'function', 'function'],
    });
  }, 120_000);
});
