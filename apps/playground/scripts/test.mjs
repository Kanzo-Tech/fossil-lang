/**
 * `pnpm --filter @fossil-lang/playground test` — the four verifiers, plus the typecheck.
 *
 * ## Why this file exists at all
 *
 * It exists because the four scripts beside it did not answer to any name a person types. They were
 * `verify-stream`, `verify-canvas`, `verify-nesting` and — until this commit — not a script at all,
 * only a file. Everything that runs anything in this repo runs `test`, so four suites asserting the
 * properties the canvas rests on were red for nobody: a change could break every one of them and
 * the only signal would be a person remembering to type four commands.
 *
 * ## Exit 2 is not a failure, and that distinction is the whole design
 *
 * Each verifier needs `public/bench/` — a million-vertex corpus, ~56 MB, gitignored, built by
 * `pnpm bench` through the `duckdb` binary. That asset is deliberately not part of `build`: a
 * playground that refuses to start because an optional demo corpus is absent would be the worse
 * app, and the same reasoning applies here. So the verifiers already separate the two answers —
 * **exit 1 is an assertion that failed, exit 2 is a precondition that was missing** — and this
 * runner keeps them separate rather than flattening both into "the tests are red".
 *
 * A missing corpus is reported as a SKIP, loudly, with the command that fixes it, and the run
 * stays green. A failed property is a failure and takes the run with it. Collapsing those two
 * would make the suite either a liar on a fresh clone or a suite people learn to ignore, and both
 * are how a green tick stops meaning anything.
 *
 * Every verifier runs even after one fails, because "which properties broke" is the question, and
 * a chain of `&&` answers only "the first one".
 */
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const app = resolve(here, '..');

/**
 * The typecheck is first and it is not a verifier.
 *
 * `tsc --noEmit` is the only thing in this list that reads `src/` rather than a corpus, and it is
 * the one that catches a source the verifiers import through `--experimental-strip-types`, which
 * strips types without checking them.
 */
const steps = [
  { name: 'typecheck', argv: ['tsc', '--noEmit'], bin: true },
  { name: 'verify-stream', argv: ['scripts/verify-stream.mjs'] },
  { name: 'verify-canvas', argv: ['scripts/verify-canvas.mjs'] },
  { name: 'verify-nesting', argv: ['scripts/verify-nesting.mjs'] },
  { name: 'verify-properties', argv: ['scripts/verify-properties.mjs'] },
];

let failed = 0;
let skipped = 0;

for (const step of steps) {
  process.stdout.write(`\n──── ${step.name} ────\n`);
  const run = step.bin
    ? spawnSync('node_modules/.bin/tsc', step.argv.slice(1), { cwd: app, stdio: 'inherit' })
    : spawnSync(process.execPath, ['--experimental-strip-types', ...step.argv], {
        cwd: app,
        stdio: 'inherit',
      });
  const code = run.status ?? 1;
  if (code === 0) continue;
  if (code === 2) {
    skipped += 1;
    process.stdout.write(`SKIP ${step.name}: no bench corpus. \`pnpm bench\` builds it (~56 MB).\n`);
    continue;
  }
  failed += 1;
  process.stdout.write(`FAIL ${step.name} (exit ${code})\n`);
}

process.stdout.write(
  `\n${steps.length - failed - skipped} passed, ${failed} failed, ${skipped} skipped\n`,
);
process.exit(failed === 0 ? 0 : 1);
