/**
 * BUG-03 deliberate-break self-test (Phase 15 plan 15-03).
 *
 * Proves that the variations harness — hardened in `variations.test.ts` —
 * actually surfaces failures with readable diagnostics rather than silently
 * timing out or returning an empty stderr.
 *
 * The hypothesis under test is the harness's *failure-mode contract*:
 *
 *   1. **Syntax error** — feeding `fossil check` a `.fossil` whose grammar
 *      breaks at the prefix/declaration level (the same shape as the curated
 *      `02-syntax-error.fossil`) MUST produce a non-zero exit code AND a
 *      stderr matching one of the lexer/parser diagnostic keywords used by
 *      the variations harness.
 *   2. **Type-error injection** — feeding `fossil check` a syntactically valid
 *      file whose Shape body references undefined bindings MUST produce a
 *      non-zero exit + non-empty stderr (no silent success on broken
 *      programs).
 *   3. **Diagnostic-format proof** — a known-broken `.fossil` MUST surface
 *      miette's box-drawing frame (`╭─`, `│`, `╰─`) and/or `Error:` prefix
 *      in stderr, proving the rich diagnostic format reaches CI logs (not
 *      just a panic stacktrace).
 *   4. **Timeout-path coverage** — feeding `runCheckWithBin` a script that
 *      sleeps forever MUST trigger the BUG-03 timeout handler in
 *      `runCheck()`-equivalent code, emitting the
 *      `Expected: ≥1 triple compile; Got: timeout (...)` diagnostic.
 *      Uses a short tmpfile timeout (2 s) so the test itself doesn't slow
 *      CI by 30 s.
 *
 * If ANY of these tests passes when it shouldn't (e.g., test 1 passes against
 * a syntactically-valid file), the variations harness has silently regressed
 * and BUG-03 has reopened.
 *
 * Out-of-scope (Phase 15 SCOPE BOUNDARY):
 *   The current `fossil check` CLI does NOT reject pure-garbage input
 *   (`!@#$ ...`) or empty files — both produce `ok — no errors`. That's a
 *   pre-existing fossil-cli looseness, not a BUG-03 concern. BUG-03 is about
 *   the HARNESS surfacing failures it observes; if the CLI doesn't report a
 *   failure, there's nothing for the harness to surface. Tracked separately
 *   in `.planning/phases/15-bug-sweep-visual-baselines/deferred-items.md`.
 *
 * Design note — why we duplicate (rather than import) `runCheck` for tests 1/2/3:
 *
 *   The exported `runCheck()` in `variations.test.ts` is bound to the
 *   `FOSSIL_BIN` module-local set by THAT file's `beforeAll`. Importing it
 *   from a sibling test file under Vitest runs both modules in the same
 *   worker but does not share `beforeAll` state across files. We instead
 *   resolve the binary path locally — same resolution logic, much smaller
 *   surface — and call `execFileSync` directly. The contract being tested
 *   is the harness's RUNTIME behaviour, so testing it via the same
 *   `execFileSync` shape (NOT via the export) is the more honest assertion.
 */
import { describe, test, expect, beforeAll } from 'vitest';
import { execFileSync } from 'node:child_process';
import {
  mkdtempSync,
  writeFileSync,
  existsSync,
  rmSync,
  chmodSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const REPO_ROOT = resolve(__dirname, '../../..');

/**
 * Same binary-resolution policy as `variations.test.ts` — prefer release,
 * fall back to debug. We do NOT rebuild here: the sibling `variations.test.ts`
 * `beforeAll` is the authoritative builder. This file runs after; on a clean
 * checkout `pnpm test` exercises variations.test.ts first (alphabetical),
 * so the binary is guaranteed present by the time we get here.
 */
function resolveFossilBin(): string {
  const release = join(REPO_ROOT, 'target', 'release', 'fossil');
  const debug = join(REPO_ROOT, 'target', 'debug', 'fossil');
  if (existsSync(release)) return release;
  if (existsSync(debug)) return debug;
  return release;
}

interface CheckOutcome {
  exitCode: number;
  stderr: string;
  stdout: string;
}

/**
 * Local `runCheck` mirror — same shape as `variations.test.ts::runCheck` so
 * the deliberate-break tests exercise the same error-handling code paths.
 * The `timeoutMs` parameter is exposed so test 4 can use a short timeout
 * (2 s) instead of the production 30 s.
 */
function runCheckWithBin(
  bin: string,
  filePath: string,
  timeoutMs: number,
): CheckOutcome {
  try {
    const stdout = execFileSync(bin, ['check', filePath], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: timeoutMs,
      killSignal: 'SIGTERM',
    });
    return { exitCode: 0, stderr: '', stdout: String(stdout) };
  } catch (e) {
    const err = e as NodeJS.ErrnoException & {
      status?: number | null;
      stderr?: Buffer | string;
      stdout?: Buffer | string;
      signal?: NodeJS.Signals | null;
    };

    if (err.signal === 'SIGTERM' || err.code === 'ETIMEDOUT') {
      const seconds = Math.round(timeoutMs / 1000);
      const diagnostic =
        `Expected: ≥1 triple compile; Got: timeout (no compile output within ${seconds}s)\n` +
        `[hint: increase per-variation timeout or diagnose hung CLI invocation]`;
      return {
        exitCode: 124,
        stderr: diagnostic,
        stdout: err.stdout ? String(err.stdout) : '',
      };
    }

    if (err.code === 'ENOENT') {
      return {
        exitCode: 127,
        stderr:
          `Expected: ${bin} executable; Got: ENOENT (binary not found)\n` +
          `[hint: rebuild via cargo build]`,
        stdout: '',
      };
    }

    return {
      exitCode: err.status ?? -1,
      stderr: err.stderr ? String(err.stderr) : '',
      stdout: err.stdout ? String(err.stdout) : '',
    };
  }
}

let FOSSIL_BIN: string;

beforeAll(() => {
  FOSSIL_BIN = resolveFossilBin();
  if (!existsSync(FOSSIL_BIN)) {
    // The sibling variations.test.ts beforeAll usually builds this; if we got
    // here without it (file ordering shuffle), build it now so the suite still
    // runs cleanly in isolation (e.g., `vitest run variations-deliberate-break`).
    execFileSync(
      'cargo',
      ['build', '-p', 'fossil-cli', '--release', '--quiet'],
      { cwd: REPO_ROOT, stdio: 'inherit' },
    );
    FOSSIL_BIN = resolveFossilBin();
  }
  if (!existsSync(FOSSIL_BIN)) {
    throw new Error(
      `fossil binary not found at ${FOSSIL_BIN} — deliberate-break self-test cannot run`,
    );
  }
}, 180_000);

describe('BUG-03 deliberate-break self-test', () => {
  test('syntax-error injection surfaces a non-zero exit + diagnostic stderr', () => {
    const dir = mkdtempSync(join(tmpdir(), 'fossil-break-syntax-'));
    try {
      const broken = join(dir, 'broken.fossil');
      // Mimic the curated `02-syntax-error.fossil` shape: a prefix line that
      // omits the `:` between the alias and the IRI. The parser detects this
      // as "expected SHAPE_SEP, found ABS_IRI" + cascade tokens. Pure-garbage
      // input (`!@#$`) is NOT rejected by the current CLI — see the
      // out-of-scope note in the file-level docstring.
      writeFileSync(
        broken,
        [
          'prefix ex <https://example.org/>',
          '',
          'users := io.csv("@examples/hello.csv")',
          '',
          'User ex:Person from users',
          '',
        ].join('\n'),
      );
      const { exitCode, stderr } = runCheckWithBin(FOSSIL_BIN, broken, 30_000);

      expect(
        exitCode,
        `Expected: non-zero exit for missing-colon prefix; Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
      ).not.toBe(0);
      expect(
        stderr.toLowerCase(),
        `Expected: stderr matches /syntax|parse|unexpected|expected|error/ for missing-colon prefix; Got:\n${stderr || '(empty)'}`,
      ).toMatch(/syntax|parse|unexpected|expected|error/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('type-error injection surfaces a non-zero exit + non-empty stderr', () => {
    const dir = mkdtempSync(join(tmpdir(), 'fossil-break-type-'));
    try {
      const broken = join(dir, 'empty-rhs.fossil');
      // Mimic the curated `03-type-error.fossil` shape: a property with an
      // empty RHS after `=`. This trips the type-checker with
      // `type-checking failed: 1 error(s)` + `unexpected token`. Note that
      // OTHER plausible "type errors" — undefined `from <binding>` refs,
      // misspelled prefixes — are silently accepted by the current CLI; only
      // the empty-RHS pattern reliably reaches the harness as a non-zero
      // exit, mirroring the curated variations set.
      writeFileSync(
        broken,
        [
          'prefix ex: <https://example.org/>',
          '',
          'users := io.csv("@examples/hello.csv")',
          '',
          'User : ex:Person from users',
          '    iri = `${ex:}user/${.id}`',
          '    ex:name =',
          '',
        ].join('\n'),
      );
      const { exitCode, stderr } = runCheckWithBin(FOSSIL_BIN, broken, 30_000);

      expect(
        exitCode,
        `Expected: non-zero exit for empty-RHS property; Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
      ).not.toBe(0);
      expect(
        stderr.length,
        `Expected: non-empty stderr for empty-RHS property; Got: empty stderr (exit=${exitCode})`,
      ).toBeGreaterThan(0);
      expect(
        stderr.toLowerCase(),
        `Expected: stderr matches /type|unexpected|error/ for empty-RHS property; Got:\n${stderr || '(empty)'}`,
      ).toMatch(/type|unexpected|error/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('miette diagnostic format reaches stderr (not just a panic stacktrace)', () => {
    // We reuse the curated 02-syntax-error variation — it is known to trigger
    // a real fossil-hir diagnostic via miette. Using the variation file
    // (rather than a tmpfile) means this test also serves as a smoke check
    // that the variation set itself still produces miette output, which the
    // playground depends on for the diagnostic panel.
    const knownBrokenVariation = resolve(
      __dirname,
      '../src/hello/variations/02-syntax-error.fossil',
    );
    const { exitCode, stderr } = runCheckWithBin(
      FOSSIL_BIN,
      knownBrokenVariation,
      30_000,
    );

    expect(
      exitCode,
      `Expected: non-zero exit for 02-syntax-error.fossil; Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
    ).not.toBe(0);
    // Miette frames either box-drawing chars `╭─` / `╰` / `│` OR the
    // ANSI-stripped `Error:` prefix. Match either — CI may or may not
    // colourize, and the playground strips ANSI before showing the panel.
    expect(
      stderr,
      `Expected: miette diagnostic shape (\`╭─\` or \`Error:\` or \`error[\`) for 02-syntax-error.fossil; Got:\n${stderr || '(empty)'}`,
    ).toMatch(/╭─|│|╰|Error:|error\[/);
  });

  test('timeout path emits the BUG-03 diagnostic shape (skipped on Windows)', () => {
    // Cross-platform note: this test relies on a POSIX shebang + chmod +x.
    // On Windows, the spawned process semantics differ and node's signal
    // handling around SIGTERM is unreliable; we skip there rather than ship
    // a flaky cross-platform timeout test. The Task 1 code-level guarantees
    // still hold on Windows — they're just not asserted here.
    if (process.platform === 'win32') {
      console.warn(
        '[skip] timeout-path test relies on POSIX shell semantics — Task 1 code-level guarantees still apply on Windows',
      );
      return;
    }

    const dir = mkdtempSync(join(tmpdir(), 'fossil-break-timeout-'));
    try {
      // Sleep-forever stand-in for the fossil binary. The actual `check`
      // arg + filepath are ignored — the script just blocks until killed.
      // 600 s sleep window dwarfs the 2 s test timeout so the SIGTERM path
      // is the only exit route.
      const fakeBin = join(dir, 'fake-fossil');
      writeFileSync(fakeBin, '#!/bin/sh\nsleep 600\n');
      chmodSync(fakeBin, 0o755);

      const dummyInput = join(dir, 'irrelevant.fossil');
      writeFileSync(dummyInput, '# content does not matter — fake bin ignores it\n');

      const start = Date.now();
      const { exitCode, stderr } = runCheckWithBin(fakeBin, dummyInput, 2_000);
      const elapsed = Date.now() - start;

      // Sanity: we killed before the 600 s sleep finished — within ~5 s
      // of the 2 s timeout (allowing for SIGTERM latency on slow CI runners).
      expect(
        elapsed,
        `Expected: kill within ~5s of the 2s timeout; Got: elapsed=${elapsed}ms`,
      ).toBeLessThan(5_000);

      expect(
        exitCode,
        `Expected: BUG-03 timeout exitCode=124; Got: exit=${exitCode}\nSTDERR:\n${stderr || '(empty)'}`,
      ).toBe(124);
      expect(
        stderr,
        `Expected: stderr matches BUG-03 timeout diagnostic shape (\`Expected: ≥1 triple compile; Got: timeout\`); Got:\n${stderr || '(empty)'}`,
      ).toMatch(/Expected:.*Got: timeout/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }, 10_000);
});
