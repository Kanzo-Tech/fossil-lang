/**
 * PLAY-05 variations harness — Phase 9 plan 09-09 Task 2.
 *
 * For each example directory under `packages/examples/src/` that has a
 * `variations/` subdirectory, this harness compiles every `.fossil` file via
 * the native `fossil check` CLI and asserts the outcome based on the file's
 * naming convention:
 *
 *   01-base.fossil          → exit 0
 *   02-syntax-error.fossil  → exit non-zero; stderr matches /syntax|parse|unexpected|expected/
 *   03-type-error.fossil    → exit non-zero; stderr matches /type|unbound|undefined|unresolved|undeclared/
 *   05-rename.fossil        → exit 0
 *   06-add-prefix.fossil    → exit 0
 *
 * Why the native CLI (Option A from the plan, not WASM-in-Node):
 *
 *   - The Phase 8 WASM (`@fossil-lang/wasm`) targets the browser; loading it
 *     under Node Vitest needs a JSDOM/`Worker` polyfill stack we'd rather
 *     not maintain inside a fixture-test package.
 *   - The native CLI is a hard dependency of CI anyway (cargo test runs in
 *     the same workflow), so spawning it from Vitest is zero new infra.
 *   - The CLI's exit code + stderr surface IS the user-facing contract for
 *     `fossil check`; testing through it catches regressions in the actual
 *     compile pipeline (parser + name resolution + type checker), not just
 *     the WASM façade.
 *
 * Why we omit the `04-shex-mismatch` variation:
 *
 *   The current `fossil-cli check` does not load CSVW descriptors, so a
 *   ShEx target mismatch on `.field` types cannot be triggered without
 *   declaring the source row schema. Variation 04-* is intentionally
 *   absent from the curated set; the 5 mutations we DO ship (1 base + 4
 *   real mutations) × 6 examples = 30 compile-gate cases per PR, which
 *   discharges SC#2's "≥30 compile checks" requirement.
 */
import { describe, test, expect, beforeAll } from 'vitest';
import { execFileSync } from 'node:child_process';
import { readdirSync, statSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

const REPO_ROOT = resolve(__dirname, '../../..');
const EXAMPLES_DIR = resolve(__dirname, '../src');

// Resolve the fossil-cli binary path — prefer release if present, else fall
// back to debug (slower but always available in dev). CI builds release in
// the workflow's "Build fossil-cli" step before running the harness; locally
// we tolerate either.
function resolveFossilBin(): string {
  const release = join(REPO_ROOT, 'target', 'release', 'fossil');
  const debug = join(REPO_ROOT, 'target', 'debug', 'fossil');
  if (existsSync(release)) return release;
  if (existsSync(debug)) return debug;
  // Neither exists — caller (beforeAll) will build one.
  return release;
}

let FOSSIL_BIN: string;

// Build the CLI once before all tests so the per-test overhead is just the
// process spawn + check. We build --release because the harness runs ~30
// invocations and the per-call ~0.5s debug overhead would add up.
beforeAll(() => {
  // Try release first; build only if missing. Skipping the rebuild when the
  // binary already exists keeps the local watch loop fast.
  FOSSIL_BIN = resolveFossilBin();
  if (!existsSync(FOSSIL_BIN)) {
    execFileSync(
      'cargo',
      ['build', '-p', 'fossil-cli', '--release', '--quiet'],
      { cwd: REPO_ROOT, stdio: 'inherit' },
    );
    FOSSIL_BIN = resolveFossilBin();
  }
  if (!existsSync(FOSSIL_BIN)) {
    throw new Error(
      `fossil binary not found after build at ${FOSSIL_BIN} — variations harness cannot run`,
    );
  }
}, 180_000);

type ExpectedOutcome = 'pass' | 'syntax-error' | 'type-error' | 'shex-mismatch';

function classifyFromFilename(name: string): ExpectedOutcome {
  if (name.includes('syntax-error')) return 'syntax-error';
  if (name.includes('type-error')) return 'type-error';
  if (name.includes('shex-mismatch')) return 'shex-mismatch';
  return 'pass';
}

interface CheckOutcome {
  exitCode: number;
  stderr: string;
  stdout: string;
}

function runCheck(filePath: string): CheckOutcome {
  try {
    const stdout = execFileSync(FOSSIL_BIN, ['check', filePath], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    return { exitCode: 0, stderr: '', stdout: String(stdout) };
  } catch (e) {
    // execFileSync throws on non-zero exit; the thrown object carries
    // .status (exit code) and .stderr (Buffer).
    const err = e as NodeJS.ErrnoException & {
      status?: number;
      stderr?: Buffer | string;
      stdout?: Buffer | string;
    };
    return {
      exitCode: err.status ?? -1,
      stderr: err.stderr ? String(err.stderr) : '',
      stdout: err.stdout ? String(err.stdout) : '',
    };
  }
}

// Discover every example dir that has a `variations/` subdir. Stable
// alphabetical order so test output is deterministic.
const exampleDirs = readdirSync(EXAMPLES_DIR)
  .filter((name) => {
    const full = join(EXAMPLES_DIR, name);
    if (!statSync(full).isDirectory()) return false;
    return existsSync(join(full, 'variations'));
  })
  .sort();

for (const example of exampleDirs) {
  describe(`PLAY-05 variations: ${example}`, () => {
    const variationsDir = join(EXAMPLES_DIR, example, 'variations');
    const variationFiles = readdirSync(variationsDir)
      .filter((f) => f.endsWith('.fossil'))
      .sort();

    for (const file of variationFiles) {
      const expected = classifyFromFilename(file);
      const filePath = join(variationsDir, file);

      test(`${file} → ${expected}`, () => {
        const { exitCode, stderr } = runCheck(filePath);
        switch (expected) {
          case 'pass':
            expect(
              exitCode,
              `expected pass but check failed:\nSTDERR:\n${stderr}`,
            ).toBe(0);
            break;
          case 'syntax-error':
            expect(exitCode, `expected fail but check passed`).not.toBe(0);
            expect(stderr.toLowerCase()).toMatch(
              /syntax|parse|unexpected|expected/,
            );
            break;
          case 'type-error':
            expect(exitCode, `expected fail but check passed`).not.toBe(0);
            expect(stderr.toLowerCase()).toMatch(
              /type|unbound|undefined|unresolved|undeclared/,
            );
            break;
          case 'shex-mismatch':
            expect(exitCode, `expected fail but check passed`).not.toBe(0);
            expect(stderr.toLowerCase()).toMatch(/shex|shape|cardinality/);
            break;
        }
      }, 30_000);
    }
  });
}
