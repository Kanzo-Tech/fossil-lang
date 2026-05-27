import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  examples,
  buildResolverExamples,
  helloExample,
  type Example,
} from '../src/index.js';

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

  it('helloExample carries a ShEx target shape', () => {
    expect(helloExample.shex).toBeDefined();
    expect(helloExample.shex!).toMatch(/PREFIX ex:/);
    expect(helloExample.shex!).toMatch(/ex:Person/);
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
