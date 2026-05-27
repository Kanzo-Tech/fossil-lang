import { describe, it, expect, beforeAll } from 'vitest';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import {
  examples,
  buildResolverExamples,
  helloExample,
  type Example,
} from '../src/index.js';

const REPO_ROOT = resolve(__dirname, '../../..');
const EXAMPLES_DIR = resolve(__dirname, '../src');

// Resolve the fossil-cli binary — release preferred, debug fallback (mirrors
// variations.test.ts contract).
function resolveFossilBin(): string {
  const release = join(REPO_ROOT, 'target', 'release', 'fossil');
  const debug = join(REPO_ROOT, 'target', 'debug', 'fossil');
  if (existsSync(release)) return release;
  if (existsSync(debug)) return debug;
  return release;
}

let FOSSIL_BIN: string;

// Build the CLI once before any audit-smoke runs. Variations harness uses the
// same pattern; if both files run in the same vitest session the cached binary
// is reused.
beforeAll(() => {
  FOSSIL_BIN = resolveFossilBin();
  if (!existsSync(FOSSIL_BIN)) {
    execFileSync('cargo', ['build', '-p', 'fossil-cli', '--release', '--quiet'], {
      cwd: REPO_ROOT,
      stdio: 'inherit',
    });
    FOSSIL_BIN = resolveFossilBin();
  }
  if (!existsSync(FOSSIL_BIN)) {
    throw new Error(
      `fossil binary not found after build at ${FOSSIL_BIN} — audit smoke cannot run`,
    );
  }
}, 180_000);

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

describe('@fossil-lang/examples', () => {
  it('exports at least one example', () => {
    expect(examples.length).toBeGreaterThan(0);
  });

  it('exports the canonical hello example as the first registered example', () => {
    // hello is the canonical 10-second walking-skeleton — must remain
    // examples[0] so PlaygroundHost can mount it as the default seed.
    expect(examples[0]).toBe(helloExample);
    // Phase 9 PLAY-05 grew the curated set from 1 → 6 examples.
    expect(examples.length).toBeGreaterThanOrEqual(6);
  });

  it('helloExample has all required Example fields populated', () => {
    const ex: Example = helloExample;
    expect(ex.id).toBe('hello');
    expect(ex.title.length).toBeGreaterThan(0);
    expect(ex.description.length).toBeGreaterThan(0);
    expect(ex.mapping.length).toBeGreaterThan(0);
    expect(ex.dataFiles.length).toBeGreaterThan(0);
  });

  it('helloExample mapping uses @examples/ paths (CONN-03 / ADR-0029)', () => {
    expect(helloExample.mapping).toMatch(/@examples\//);
    // And explicitly NOT the cargo-side file path:
    expect(helloExample.mapping).not.toMatch(/examples\/users\.csv/);
  });

  it('helloExample has a CSV data file with header + at least one row', () => {
    const csv = helloExample.dataFiles.find(f => f.format === 'csv');
    expect(csv).toBeDefined();
    expect(csv!.path).toBe('hello.csv');
    const lines = csv!.contents.split('\n').filter(l => l.length > 0);
    expect(lines.length).toBeGreaterThanOrEqual(2); // header + ≥1 data row
    expect(lines[0]).toMatch(/,/);
  });

  it('helloExample no longer carries a CSVW JSON-LD descriptor (Phase 13 / ADR-0037)', () => {
    // Phase 13 v0.2 (ADR-0037 / plan 13-04b) dropped user-facing CSVW. The
    // hello example no longer ships a sidecar; schema is inferred at compile
    // time via host-side DuckDB DESCRIBE (`useInferredDescriptors` hook).
    expect(helloExample.csvw).toBeUndefined();
  });

  it('helloExample carries a ShEx target shape (JSON-LD form)', () => {
    // Phase 15 plan 15-02 (BUG-02): the sibling .shex was converted from
    // ShExC compact syntax to ShEx 2.1 JSON-LD so the CLI's
    // `ShExDescriptor::from_reader` (JSON-LD only) auto-discovery succeeds.
    expect(helloExample.shex).toBeDefined();
    expect(helloExample.shex!).toMatch(/"@context"/);
    expect(helloExample.shex!).toMatch(/shex\.jsonld/);
    expect(helloExample.shex!).toMatch(/example\.org\/Person/);
  });

  it('buildResolverExamples maps data file paths → contents', () => {
    const resolver = buildResolverExamples();
    expect(resolver['hello.csv']).toBeDefined();
    expect(resolver['hello.csv']!.length).toBeGreaterThan(0);
    expect(resolver['hello.csv']).toBe(helloExample.dataFiles[0]!.contents);
  });

  it('buildResolverExamples includes the CSVW + ShEx descriptors keyed by id', () => {
    const resolver = buildResolverExamples();
    expect(resolver['hello.csvw.json']).toBe(helloExample.csvw);
    expect(resolver['hello.shex']).toBe(helloExample.shex);
  });

  it('buildResolverExamples does NOT include the .fossil mapping (mapping goes to editor)', () => {
    const resolver = buildResolverExamples();
    // No key should match a .fossil suffix or contain the raw mapping text.
    for (const k of Object.keys(resolver)) {
      expect(k.endsWith('.fossil')).toBe(false);
    }
    for (const v of Object.values(resolver)) {
      expect(v).not.toBe(helloExample.mapping);
    }
  });

  // BUG-02 (Phase 15 plan 15-02) audit-aware smoke: every curated example's
  // root `.fossil` source must compile end-to-end via `fossil check` and exit
  // 0. This mirrors the Phase 13 ADR-0037 inferred-CSVW contract: examples no
  // longer need a `schema = "..."` arg, and any sibling `.shex` shape must be
  // loadable by the CLI's auto-discovery (JSON-LD form per
  // `ShExDescriptor::from_reader`).
  //
  // Why this is separate from variations.test.ts: the variations harness
  // exercises files under `<example>/variations/` (which have no sibling
  // descriptors); this smoke exercises the example's ROOT `<example>.fossil`
  // (which DOES have siblings — `.csv`, `.shex`, optional `.csvw.json`). It
  // catches the auto-discovery failure mode (sibling format mismatch) the
  // variations subtree by construction can't see.
  const EXAMPLE_IDS = [
    'hello',
    'hello-no-csvw',
    'ecommerce',
    'musicbrainz',
    'typing-showcase',
    'multi-source-join',
  ] as const;

  describe('BUG-02 audit smoke: root .fossil compiles via `fossil check`', () => {
    for (const id of EXAMPLE_IDS) {
      it(`${id}/${id}.fossil → fossil check exit 0`, () => {
        const filePath = join(EXAMPLES_DIR, id, `${id}.fossil`);
        expect(existsSync(filePath), `${filePath} missing on disk`).toBe(true);
        const { exitCode, stderr } = runCheck(filePath);
        expect(
          exitCode,
          `expected exit 0 but ${id} failed:\nSTDERR:\n${stderr}`,
        ).toBe(0);
      }, 30_000);
    }
  });

  // Pinning smoke test: the package-side mapping is a copy of the cargo-side
  // example with ONE rewrite (source URI). If a future grammar change lands
  // in cargo's hello.fossil but the package copy lags, prefix decls diverge
  // and this test catches it. Loosened to compare ONLY the prefix decls —
  // strict enough to flag grammar drift, loose enough to allow the URI
  // rewrite + comment differences.
  it('package-side hello.fossil prefix decls match cargo-side examples/hello.fossil (smoke)', () => {
    // Resolve cargo-side path relative to the repo root (2 levels up from
    // packages/examples/tests/). Vitest's `cwd` is the package root.
    const cargoPath = resolve(process.cwd(), '..', '..', 'examples', 'hello.fossil');
    const cargoSrc = readFileSync(cargoPath, 'utf8');
    const cargoPrefixes = cargoSrc
      .split('\n')
      .filter(l => l.startsWith('prefix '))
      .map(l => l.trim());
    const pkgPrefixes = helloExample.mapping
      .split('\n')
      .filter(l => l.startsWith('prefix '))
      .map(l => l.trim());
    expect(pkgPrefixes).toEqual(cargoPrefixes);
    expect(pkgPrefixes.length).toBeGreaterThan(0); // sanity: at least one
  });
});
