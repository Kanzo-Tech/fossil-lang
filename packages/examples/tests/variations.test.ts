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
 *
 * BUG-03 hardening (Phase 15 plan 15-03):
 *
 *   The harness now fails LOUDLY — never silently — on the two pre-Phase-15
 *   blind spots:
 *
 *     1. **Per-variation timeout.** `execFileSync` is called with an explicit
 *        `timeout: PER_VARIATION_TIMEOUT_MS, killSignal: 'SIGTERM'`. When the
 *        child is killed, `runCheck()` returns a diagnostic-shaped
 *        `CheckOutcome` with `exitCode: 124` (POSIX timeout) and stderr
 *        containing `Expected: ≥1 triple compile; Got: timeout (no compile
 *        output within Ns)` plus a hint. The vitest-level test timeout is the
 *        SAME constant so the two clocks can't drift.
 *
 *     2. **Diagnostic-shaped expect messages.** Every `expect(exitCode).toBe(0)`
 *        (and its non-zero siblings) carries an `Expected: <file> compile pass;
 *        Got: exit=N\nSTDERR:\n<stderr>` message string so the failure surfaces
 *        in CI logs WITHOUT needing a separate `console.log` (which vitest may
 *        truncate).
 *
 *   The self-test `variations-deliberate-break.test.ts` proves the contract:
 *   if either guarantee silently regresses, that file's tests fail.
 */
import { describe, test, expect, beforeAll } from 'vitest';
import { execFileSync } from 'node:child_process';
import { readdirSync, statSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

const REPO_ROOT = resolve(__dirname, '../../..');
const EXAMPLES_DIR = resolve(__dirname, '../src');

/**
 * Single source of truth for per-variation timeout — used BOTH as the
 * `execFileSync` child-process timeout AND as the vitest `test()` third-arg
 * timeout. Keeping the two clocks in sync prevents the silent-kill failure
 * mode BUG-03 was filed against (vitest killing the test 1ms before the child
 * process had a chance to emit its diagnostic).
 */
const PER_VARIATION_TIMEOUT_MS = 30_000;

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
// process spawn + check. DEBUG build: `fossil-cli` links the whole engine
// (datafusion/duckdb), so the release `opt-level` compile dominates CI time —
// far more than the ~0.5s/call the ~30 debug invocations add back. A release
// binary (built by a workflow step) is still preferred if already present.
beforeAll(() => {
  // Prefer an existing release/debug binary; build debug only if neither
  // exists. Skipping the rebuild keeps the local watch loop fast.
  FOSSIL_BIN = resolveFossilBin();
  if (!existsSync(FOSSIL_BIN)) {
    execFileSync(
      'cargo',
      ['build', '-p', 'fossil-cli', '--quiet'],
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

/**
 * Run `fossil check <filePath>` and return a normalized `CheckOutcome`.
 *
 * Failure modes — all surface as a diagnostic-shaped `stderr`, never as a
 * silent kill or empty string:
 *
 *   - **Compile error (non-zero exit, child wrote to stderr):** captured as-is.
 *   - **Timeout (child killed by SIGTERM after `PER_VARIATION_TIMEOUT_MS`):**
 *     `exitCode = 124` (POSIX timeout convention), stderr starts with
 *     `Expected: ≥1 triple compile; Got: timeout (no compile output within Ns)`
 *     plus an actionable hint.
 *   - **Binary not found (ENOENT — should be unreachable post-`beforeAll`):**
 *     `exitCode = 127` (POSIX command-not-found convention), stderr names the
 *     FOSSIL_BIN path that was attempted.
 *
 * Exported so the deliberate-break self-test (`variations-deliberate-break.test.ts`)
 * can exercise the exact same code path it's asserting against.
 */
export function runCheck(filePath: string): CheckOutcome {
  try {
    const stdout = execFileSync(FOSSIL_BIN, ['check', filePath], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: PER_VARIATION_TIMEOUT_MS,
      killSignal: 'SIGTERM',
    });
    return { exitCode: 0, stderr: '', stdout: String(stdout) };
  } catch (e) {
    // execFileSync throws on non-zero exit, on timeout (signal === 'SIGTERM'
    // / code === 'ETIMEDOUT'), and on spawn failure (code === 'ENOENT').
    // The thrown object carries .status (exit code), .stderr (Buffer),
    // .stdout (Buffer), .signal (string|null), and .code (string|undefined).
    const err = e as NodeJS.ErrnoException & {
      status?: number | null;
      stderr?: Buffer | string;
      stdout?: Buffer | string;
      signal?: NodeJS.Signals | null;
    };

    // --- Timeout path (BUG-03 fix) ------------------------------------------
    // When execFileSync's timeout fires it kills the child with `killSignal`
    // and re-throws; the thrown error carries either signal === 'SIGTERM'
    // (most platforms) or code === 'ETIMEDOUT' (some Node versions).
    if (err.signal === 'SIGTERM' || err.code === 'ETIMEDOUT') {
      const seconds = Math.round(PER_VARIATION_TIMEOUT_MS / 1000);
      const diagnostic =
        `Expected: ≥1 triple compile; Got: timeout (no compile output within ${seconds}s)\n` +
        `[hint: increase PER_VARIATION_TIMEOUT_MS in variations.test.ts if the example legitimately needs >${seconds}s, ` +
        `or diagnose the hung CLI invocation: \`${FOSSIL_BIN} check ${filePath}\`]`;
      return {
        exitCode: 124,
        stderr: diagnostic,
        stdout: err.stdout ? String(err.stdout) : '',
      };
    }

    // --- Spawn-failure path (belt-and-suspenders) ---------------------------
    // `beforeAll` already throws on missing binary, so this branch is
    // unreachable in practice — kept defensively in case the binary is
    // deleted between `beforeAll` and a specific test (e.g., concurrent
    // `cargo clean` during a watch loop).
    if (err.code === 'ENOENT') {
      return {
        exitCode: 127,
        stderr:
          `Expected: ${FOSSIL_BIN} executable; Got: ENOENT (binary not found)\n` +
          `[hint: rebuild via \`cargo build -p fossil-cli --release\`]`,
        stdout: '',
      };
    }

    // --- Standard non-zero exit path (compile error etc.) -------------------
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
              `Expected: ${file} compile pass; Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
            ).toBe(0);
            break;
          case 'syntax-error':
            expect(
              exitCode,
              `Expected: ${file} syntax-error (non-zero exit); Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
            ).not.toBe(0);
            expect(
              stderr.toLowerCase(),
              `Expected: stderr matches /syntax|parse|unexpected|expected/ for ${file}; Got:\n${stderr || '(empty)'}`,
            ).toMatch(/syntax|parse|unexpected|expected/);
            break;
          case 'type-error':
            expect(
              exitCode,
              `Expected: ${file} type-error (non-zero exit); Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
            ).not.toBe(0);
            expect(
              stderr.toLowerCase(),
              `Expected: stderr matches /type|unbound|undefined|unresolved|undeclared/ for ${file}; Got:\n${stderr || '(empty)'}`,
            ).toMatch(/type|unbound|undefined|unresolved|undeclared/);
            break;
          case 'shex-mismatch':
            expect(
              exitCode,
              `Expected: ${file} shex-mismatch (non-zero exit); Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
            ).not.toBe(0);
            expect(
              stderr.toLowerCase(),
              `Expected: stderr matches /shex|shape|cardinality/ for ${file}; Got:\n${stderr || '(empty)'}`,
            ).toMatch(/shex|shape|cardinality/);
            break;
        }
      }, PER_VARIATION_TIMEOUT_MS);
    }
  });
}
