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
 * `./corpus` is here for the same reason and by the same method. `openCorpus` is the reference API
 * and it has to decode Parquet, which is exactly the dependency this package does not have: the
 * engine arrives as a `query` callback, so the module's own closure is still `corpus.js` plus the
 * two the addressing needs. A decoder pulled in as a dependency would not be a type error anywhere,
 * and no test but this one would notice.
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

/**
 * Compile one entry point and whatever it pulls in into a package directory with no `pkg/`, link
 * that into a consumer, and run `probe` there. Returns the probe's stdout.
 *
 * `entry` is the only root. tsc follows its imports, so whatever the closure really is is what
 * lands in `dist/` — this step is the measurement, not a copy of a list.
 */
function standalone(name: string, entry: string, probe: readonly string[]): string {
  const linked = join(work, name, 'graph');
  mkdirSync(join(linked, 'dist'), { recursive: true });

  execFileSync(
    join(packageRoot, 'node_modules', '.bin', 'tsc'),
    [
      entry,
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

  const consumer = join(work, name, 'consumer');
  mkdirSync(join(consumer, 'node_modules', '@fossil-lang'), { recursive: true });
  symlinkSync(linked, join(consumer, 'node_modules', '@fossil-lang', 'graph'), 'dir');
  // No `node_modules` beyond the link, so an import of anything the closure did not bring with it
  // dies with ERR_MODULE_NOT_FOUND naming the package — which is what "zero runtime dependencies"
  // means when it is a measurement rather than a line in `package.json`.
  writeFileSync(join(consumer, 'package.json'), '{ "type": "module" }');
  writeFileSync(join(consumer, 'probe.mjs'), probe.join('\n'));

  return execFileSync(process.execPath, ['probe.mjs'], { cwd: consumer, encoding: 'utf8' });
}

describe('@fossil-lang/graph/address — reachable without the WASM toolchain', () => {
  it('publishes the subpath the addressing half is reached by', () => {
    expect(manifest.exports['./address']).toEqual({
      types: './dist/address.d.ts',
      import: './dist/address.js',
    });
  });

  it('imports from a package directory that has no pkg/ in it', () => {
    const stdout = standalone('address', 'src/address.ts', [
      "import { CorpusManifestError, GRAPH_INFO_PATH, resolveCorpus, shiftFor, tileOf, TILE_SHIFT } from '@fossil-lang/graph/address';",
      'process.stdout.write(JSON.stringify({',
      '  tile: String(tileOf(4096n)),',
      '  shift: String(TILE_SHIFT),',
      '  info: GRAPH_INFO_PATH,',
      '  kinds: [typeof resolveCorpus, typeof shiftFor, typeof CorpusManifestError],',
      '}));',
    ]);

    expect(JSON.parse(stdout)).toEqual({
      tile: '1',
      shift: '12',
      info: GRAPH_INFO_PATH,
      kinds: ['function', 'function', 'function'],
    });
  }, 120_000);
});

describe('@fossil-lang/graph/corpus — the reference API brings no engine with it', () => {
  it('publishes the subpath openCorpus is reached by', () => {
    expect(manifest.exports['./corpus']).toEqual({
      types: './dist/corpus.d.ts',
      import: './dist/corpus.js',
    });
  });

  it('loads, and refuses to open a corpus, with nothing installed beside it', () => {
    // The probe asks for the failure rather than for a corpus: there is no engine here, which is
    // the whole point. A `query` that is not a function has to be caught by `openCorpus` itself,
    // because the alternative is a TypeError from inside a template literal three calls down.
    const stdout = standalone('corpus', 'src/corpus.ts', [
      "import { CorpusManifestError, CorpusReadError, openCorpus } from '@fossil-lang/graph/corpus';",
      'const errors = [];',
      "try { await openCorpus('https://example.org/corpus', {}); } catch (e) { errors.push(e.constructor.name); }",
      'process.stdout.write(JSON.stringify({',
      '  kinds: [typeof openCorpus, typeof CorpusManifestError, typeof CorpusReadError],',
      '  errors,',
      '}));',
    ]);

    expect(JSON.parse(stdout)).toEqual({
      kinds: ['function', 'function', 'function'],
      errors: ['TypeError'],
    });
  }, 120_000);
});
