/**
 * ONE DuckDB-WASM in the built bundle. The condition, asserted rather than remembered.
 *
 * `CLAUDE.md` names this as the property that keeps the playground's whole engine decision honest:
 * «`pnpm --filter @fossil-lang/playground... build` emitting one `duckdb-*.wasm` asset is the
 * condition that keeps it true». It used to be true by ABSTINENCE — the app declined
 * `@kanzo-tech/mosaic` entirely, because `@uwdata/mosaic-core@0.29.2` names
 * `@duckdb/duckdb-wasm@1.33.1-dev57.0` as an exact dependency against this app's `1.32.0`, and
 * taking it would have put a second engine in the tab.
 *
 * The app takes it now — there is a chart beside the canvas and the two pins are reconciled by an
 * override in the root `package.json` — so the condition is no longer maintained by not doing the
 * thing. It is maintained by a version override, which is exactly the kind of fact that decays
 * silently: a `pnpm update`, a bump of either pin, or a transitive dependency growing its own
 * `@duckdb/duckdb-wasm` range would each restore the second engine without a single line of this
 * app changing. So it is measured, here, on the artefact.
 *
 * Run after a build:
 *
 *     pnpm --filter "@fossil-lang/playground..." build
 *     node apps/playground/scripts/verify-one-engine.mjs
 *
 * It reads `dist/` and asserts nothing else — no bundler internals, no lockfile parsing. What ships
 * is what is counted, because what ships is what a reader's browser would download.
 */
import { readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, '..', 'dist');

/** Every file under a directory, recursively, as paths relative to it. */
function walk(root, prefix = '') {
  const out = [];
  for (const entry of readdirSync(join(root, prefix), { withFileTypes: true })) {
    const rel = join(prefix, entry.name);
    if (entry.isDirectory()) out.push(...walk(root, rel));
    else out.push(rel);
  }
  return out;
}

let files;
try {
  files = walk(dist);
} catch {
  console.error(
    `no dist/ at ${dist}\n` +
      'Build first — with the three dots, or the workspace dependencies are not built:\n' +
      '  pnpm --filter "@fossil-lang/playground..." build',
  );
  process.exit(2);
}

// `duckdb-*.wasm` and not `*.wasm`: the app ships fossil's own wasm too — the compiler shim, the
// executor, the graph verb surface — and those are the point rather than a duplicate.
const engines = files.filter((f) => /(^|\/)duckdb-[^/]*\.wasm$/.test(f));

const KB = (bytes) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;
for (const engine of engines) {
  console.log(`  ${engine}  ${KB(statSync(join(dist, engine)).size)}`);
}

if (engines.length === 1) {
  console.log(`\nONE duckdb wasm asset. The override holds and the coordinator runs on the app's engine.`);
  process.exit(0);
}

console.error(
  `\nExpected exactly 1 duckdb-*.wasm asset, found ${engines.length}.\n\n` +
    (engines.length === 0
      ? 'Zero means the build did not emit the engine at all — check that src/duckdb.ts still\n' +
        'imports the bundle with `?url`, which is what makes Vite emit it as an asset.\n'
      : 'Two or more means a second DuckDB-WASM re-entered the dependency graph. The usual cause\n' +
        'is the root package.json override no longer matching @uwdata/mosaic-core\'s pin. Check:\n' +
        '  node -e "console.log(require(\'./node_modules/.pnpm/@uwdata+mosaic-core@0.29.2/node_modules/@duckdb/duckdb-wasm/package.json\').version)"\n' +
        'and, before changing either pin, re-check that the connection API is still byte-identical:\n' +
        '  md5 node_modules/.pnpm/@duckdb+duckdb-wasm@*/node_modules/@duckdb/duckdb-wasm/dist/types/src/parallel/async_connection.d.ts\n' +
        'That file is the whole basis of the override — see the //pnpm note in the root manifest.\n'),
);
process.exit(1);
