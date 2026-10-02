/**
 * Repo-wide guard: the TypeScript half of `/docs/design/failure`'s guards, over every
 * `packages/<name>/src` file this repository writes by hand. The Rust half is
 * `crates/xtask/tests/failure_guards.rs`.
 *
 * # What this holds
 *
 * 1. **No failure is swallowed without a reason on the line.** An empty `catch`, a `catch` that does
 *    not bind the error, `.catch(() => {})` and its `undefined`/`null` spellings, and a `.then` whose
 *    rejection handler answers nothing, each need a `//` comment on the line, the line before, or the
 *    first line of the body.
 * 2. **No `??=` assigns a call.** `scope[KEY] ??= boot()` memoizes a rejection for the life of the
 *    page; a cache that clears itself on rejection is written out, as the three wasm loaders are.
 * 3. **Every host wait and every `fetch` goes through `within`.** A `host.connections(`,
 *    `host.credentials(`, `job.complete(` or `fetch(` call has `within(` on its line or one of the
 *    two before it.
 *
 * # Why
 *
 * The audit of 2026-10-01 found each of these in the tree — a `.catch(() => {})` on the failure
 * report that left a job `running` forever, three loaders that memoized a dropped download, ten host
 * waits with no end. The rule is prose on `/docs/design/failure`, and prose does not go red.
 *
 * # What this cannot prove
 *
 * - **That the reason is true.** A comment satisfies rule 1 whatever it says.
 * - **A memoization spelled another way.** `if (!p) p = boot()` with no clearing passes rule 2;
 *   the loaders' `pending.catch(…)` is the shape a reviewer looks for.
 * - **That the figure is right**, or that `within` wraps the call rather than sitting near it:
 *   rule 3 reads lines, not the syntax tree. A host reached under another name — `io.host.x(` is
 *   caught, `const h = host; h.x(` is not — escapes it.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const PACKAGES = fileURLToPath(new URL('../../', import.meta.url));

function sources(): { path: string; lines: string[] }[] {
  const out: { path: string; lines: string[] }[] = [];
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const at = join(dir, name);
      if (statSync(at).isDirectory()) walk(at);
      else if (name.endsWith('.ts') && !/\.gen\.ts$|\.generated\.ts$|\.d\.ts$/.test(name)) {
        out.push({ path: relative(PACKAGES, at), lines: readFileSync(at, 'utf8').split('\n') });
      }
    }
  };
  for (const pkg of readdirSync(PACKAGES)) {
    const src = join(PACKAGES, pkg, 'src');
    try {
      if (statSync(src).isDirectory()) walk(src);
    } catch {
      // A package with no `src` has nothing for this guard to read.
      continue;
    }
  }
  return out;
}

const SWALLOW = [
  /catch\s*\{/, // a catch that does not bind the error
  /catch\s*\([^)]*\)\s*\{\s*\}/, // an empty catch
  /\.catch\(\s*\(\)\s*=>\s*(\{\s*\}|undefined|null)\s*\)/,
  /,\s*\(\)\s*=>\s*(undefined|\{\s*\}|null)\s*,?\s*\)/, // a `.then(…, () => undefined)`
];
const commented = (line: string | undefined): boolean => line !== undefined && /\/\/|^\s*\*|\/\*/.test(line);

describe('the failure guards', () => {
  const files = sources();

  it('reads the packages', () => {
    expect(files.some((f) => f.path === 'executor/src/run.ts')).toBe(true);
  });

  it('swallows no failure without a reason on the line', () => {
    const bare: string[] = [];
    for (const { path, lines } of files) {
      lines.forEach((line, i) => {
        if (!SWALLOW.some((re) => re.test(line))) return;
        if ([line, lines[i - 1], lines[i + 1]].some(commented)) return;
        bare.push(`${path}:${i + 1}: ${line.trim()}`);
      });
    }
    expect(bare).toEqual([]);
  });

  it('assigns no call with ??=, so no rejection is memoized', () => {
    const memoized = files.flatMap(({ path, lines }) =>
      lines.flatMap((line, i) => (/\?\?=\s*(await\s+)?[\w.$]+\s*\(|\?\?=\s*new\s+Promise/.test(line) ? [`${path}:${i + 1}: ${line.trim()}`] : [])),
    );
    expect(memoized).toEqual([]);
  });

  it('waits on no host and fetches nothing outside within()', () => {
    const unbounded: string[] = [];
    for (const { path, lines } of files) {
      lines.forEach((line, i) => {
        if (!/\bhost\.(connections|credentials)\(|\bjob\.complete\(|(^|[^\w.])fetch\(/.test(line)) return;
        if (lines.slice(Math.max(0, i - 2), i + 1).some((l) => l.includes('within('))) return;
        unbounded.push(`${path}:${i + 1}: ${line.trim()}`);
      });
    }
    expect(unbounded).toEqual([]);
  });
});
