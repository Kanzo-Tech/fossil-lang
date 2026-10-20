// The benchmark's second fossil host: the program through `@fossil-lang/executor` in Node,
// which is how fossil runs in production — the wasm32 build, one thread, a linear memory that
// ends at 4 GiB. It does what `fossil-driver` does natively, so the two rows of a table differ
// only in the build:
//
//   node benches/engines/fossil-node.mjs <dir> <program.fossil> <out>
//
// Every file under <dir> is handed to `run` as bytes at a synthetic URL, the graph comes back in
// memory and is written to <out>. One JSON line on stdout: `serve_ms`, `run_ms`, `write_ms`,
// `files`, `bytes`, and `wasm_memory_bytes`, the linear memory the module ended with.
//
// It needs `packages/executor` built (`pnpm --filter @fossil-lang/executor build`), and runs
// with Node from the repository so the workspace package resolves.

import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, relative } from 'node:path';
import { performance } from 'node:perf_hooks';

const [dir, program, out] = process.argv.slice(2);
if (!out) {
  console.error('usage: fossil-node.mjs <dir> <program.fossil> <out>');
  process.exit(2);
}

const require = createRequire(join(import.meta.dirname, '../../packages/executor/package.json'));
const { initFossilExecutor, run } = await import(require.resolve('@fossil-lang/executor'));
const module = await initFossilExecutor(
  readFileSync(require.resolve('@fossil-lang/executor/pkg/fossil_df_wasm_bg.wasm')),
);

const base = 'https://programs.test/';

function under(at) {
  return readdirSync(at).flatMap((name) => {
    const path = join(at, name);
    return statSync(path).isDirectory() ? under(path) : [path];
  });
}

let started = performance.now();
const files = {};
for (const path of under(dir)) {
  const rel = relative(dir, path).split('\\').join('/');
  if (!rel.startsWith('out/')) files[`${base}${rel}`] = readFileSync(path);
}
const serve = performance.now() - started;

started = performance.now();
const ran = await run(readFileSync(join(dir, program), 'utf8'), { files, path: `${base}${program}` });
const runMs = performance.now() - started;

started = performance.now();
let bytes = 0;
for (const file of ran.files) {
  const path = join(out, file.path);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, file.bytes);
  bytes += file.bytes.byteLength;
}
const write = performance.now() - started;

console.log(
  JSON.stringify({
    serve_ms: serve,
    run_ms: runMs,
    write_ms: write,
    files: ran.files.length,
    bytes,
    wasm_memory_bytes: module?.memory?.buffer?.byteLength ?? null,
  }),
);
