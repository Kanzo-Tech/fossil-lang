/**
 * Each fossil package's `.wasm` is a bundler ASSET: emitted by a real build, and loaded from where
 * it was emitted, with the host naming no URL.
 *
 * The three loaders take no argument in a bundled host. The wasm-bindgen glue resolves
 * `new URL('<name>_bg.wasm', import.meta.url)` and a bundler is expected to see that pattern, copy
 * the file into its output and rewrite the URL. That is a claim about a bundler, so it is checked
 * against one: each package is built through Vite's production pipeline from an entry that imports
 * it by its package name — the way a host does, through `dist/` — and the output is then executed.
 *
 * Asserted per package:
 *  1. exactly one `<name>_bg-<hash>.wasm` is emitted, byte-identical to `pkg/<name>_bg.wasm`;
 *  2. importing the built chunk boots the module with `init()` called with nothing, the one
 *     `.wasm` it fetches is that emitted file, and one real call answers through it.
 *
 * Step 2 runs in Node, whose `fetch` rejects `file://` — so `fetch` is lent a `file:` branch for
 * this process and nothing else. The URL it is asked for is the one the BUILD wrote, which is the
 * thing under test.
 *
 * Exit 1 is a failed assertion. Needs `pkg/` and `dist/` of the three packages:
 *
 *     pnpm --filter "@fossil-lang/playground..." build
 */
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'vite';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');
const repo = resolve(app, '..', '..');

/** One entry per package: what it imports, and the one call that proves the module is up. */
const cases = [
  {
    pkg: 'wasm',
    stem: 'fossil_wasm_bg',
    entry: `import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
await initFossilWasm();
export const answer = tokenize('User := io.csv("people.csv")').length;`,
    check: (answer) => typeof answer === 'number' && answer > 0,
  },
  {
    pkg: 'executor',
    stem: 'fossil_df_wasm_bg',
    entry: `import { initFossilExecutor, FossilExecutor } from '@fossil-lang/executor';
await initFossilExecutor();
export const answer = Array.isArray(new FossilExecutor('x := 1').missingDocuments());`,
    check: (answer) => answer === true,
  },
  {
    pkg: 'corpus',
    stem: 'fossil_graph_wasm_bg',
    // An empty manifest set is refused by `fossil_graph::plan` — which it can only do once the
    // module is up. A boot failure would surface as a different error.
    entry: `import { open, CorpusManifestError } from '@fossil-lang/corpus';
export const answer = await open('/c', { manifestFiles: {} }).then(
  () => 'opened',
  (e) => (e instanceof CorpusManifestError ? 'refused by the manifest' : String(e)),
);`,
    check: (answer) => answer === 'refused by the manifest',
  },
];

const realFetch = globalThis.fetch;
/** Every `file:` URL fetched — the build's rewrite of `import.meta.url`, observed. */
const fetched = [];
globalThis.fetch = async (input, init) => {
  const url = input instanceof Request ? input.url : String(input);
  if (!url.startsWith('file:')) return realFetch(input, init);
  fetched.push(fileURLToPath(url));
  return new Response(readFileSync(fileURLToPath(url)), {
    headers: { 'content-type': 'application/wasm' },
  });
};

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
// Under node_modules/ so the entries resolve `@fossil-lang/*` from this app, as a host's would.
const srcDir = join(app, 'node_modules', '.verify-wasm-assets');
const outRoot = realpathSync(mkdtempSync(join(tmpdir(), 'fossil-wasm-assets-')));
let failed = 0;

try {
  mkdirSync(srcDir, { recursive: true });
  for (const c of cases) {
    const entry = join(srcDir, `${c.pkg}.js`);
    writeFileSync(entry, c.entry);
    const outDir = join(outRoot, c.pkg);
    await build({
      root: app,
      configFile: false,
      logLevel: 'warn',
      base: './',
      build: {
        outDir,
        emptyOutDir: true,
        target: 'esnext',
        minify: false,
        rollupOptions: {
          input: entry,
          preserveEntrySignatures: 'strict',
          output: { entryFileNames: 'entry.js' },
        },
      },
    });

    const emitted = readdirSync(join(outDir, 'assets')).filter(
      (f) => f.startsWith(`${c.stem}-`) && f.endsWith('.wasm'),
    );
    const source = readFileSync(join(repo, 'packages', c.pkg, 'pkg', `${c.stem}.wasm`));
    fetched.length = 0;
    let answer;
    try {
      ({ answer } = await import(pathToFileURL(join(outDir, 'entry.js')).href));
    } catch (e) {
      answer = `threw: ${e}`;
    }
    const identical =
      emitted.length === 1 && sha(readFileSync(join(outDir, 'assets', emitted[0]))) === sha(source);
    const fromAsset =
      fetched.length === 1 && emitted.length === 1 && fetched[0] === join(outDir, 'assets', emitted[0]);
    const ok = identical && fromAsset && c.check(answer);
    if (!ok) failed += 1;
    console.log(
      `${ok ? 'ok  ' : 'FAIL'} @fossil-lang/${c.pkg}: emitted ${JSON.stringify(emitted)}` +
        `${identical ? ' (= pkg/)' : ' (NOT the pkg/ bytes, or not exactly one)'}, ` +
        `fetched ${fromAsset ? 'that file' : JSON.stringify(fetched)}, init() → ${JSON.stringify(answer)}`,
    );
  }
} finally {
  rmSync(srcDir, { recursive: true, force: true });
  rmSync(outRoot, { recursive: true, force: true });
}

process.exit(failed === 0 ? 0 : 1);
