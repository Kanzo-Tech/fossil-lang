import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { CorpusManifestError, FOSSIL_FORMAT, parseManifest } from '../src/manifest.js';

/**
 * `fossil.json`: what the reader refuses, and the shape every fixture it is tested on has.
 *
 * ── MERGE HOOK ──────────────────────────────────────────────────────────────────────────────────
 * The writer's `fossil_sinks::manifest::Manifest` is the source of the shape, and `cargo xtask`
 * will emit a JSON Schema from it. Until that schema exists, `shapeOf` below is the contract
 * (`fossil/1` §2) written as a check, and `src/manifest.ts` is typed to it by hand. When the
 * schema lands: validate every fixture below against it instead of `shapeOf`, and hold
 * `src/manifest.ts`'s types against it (or generate them), then delete `shapeOf`. This block is the
 * one place that changes.
 * ────────────────────────────────────────────────────────────────────────────────────────────────
 */

/** Every way `m` departs from the contract's shape, as sentences. Empty when it conforms. */
function shapeOf(m: unknown): string[] {
  const out: string[] = [];
  const obj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
  const str = (v: unknown, what: string) => typeof v === 'string' || out.push(`${what} is not a string`);
  const optStr = (v: unknown, what: string) => v === undefined || str(v, what);
  const count = (v: unknown, what: string) =>
    (Number.isSafeInteger(v) && (v as number) >= 0) || out.push(`${what} is not a count`);
  const properties = (v: unknown, what: string) => {
    if (!Array.isArray(v)) return out.push(`${what}.properties is not a list`);
    v.forEach((p, i) => {
      if (!obj(p)) return out.push(`${what}.properties[${i}] is not an object`);
      str(p.name, `${what}.properties[${i}].name`);
      str(p.type, `${what}.properties[${i}].type`);
      optStr(p.iri, `${what}.properties[${i}].iri`);
      if (p.nullable !== undefined && typeof p.nullable !== 'boolean') out.push(`${what}.properties[${i}].nullable is not a bool`);
    });
  };
  if (!obj(m)) return ['the manifest is not an object'];
  if (m.format !== FOSSIL_FORMAT) out.push(`format is ${JSON.stringify(m.format)}`);
  if (!Array.isArray(m.vertex_tables)) out.push('vertex_tables is not a list');
  if (!Array.isArray(m.edge_tables)) out.push('edge_tables is not a list');
  for (const [i, t] of ((m.vertex_tables as unknown[]) ?? []).entries()) {
    const what = `vertex_tables[${i}]`;
    if (!obj(t)) {
      out.push(`${what} is not an object`);
      continue;
    }
    for (const k of ['name', 'path', 'key', 'identity']) str(t[k], `${what}.${k}`);
    optStr(t.iri, `${what}.iri`);
    count(t.record_count, `${what}.record_count`);
    properties(t.properties, what);
    if (t.position !== undefined) {
      const p = t.position;
      if (!obj(p) || (p.by !== 'layout' && p.by !== 'program') || typeof p.x !== 'string' || typeof p.y !== 'string') {
        out.push(`${what}.position is not { by: layout | program, x, y }`);
      }
    }
  }
  for (const [i, t] of ((m.edge_tables as unknown[]) ?? []).entries()) {
    const what = `edge_tables[${i}]`;
    if (!obj(t)) {
      out.push(`${what} is not an object`);
      continue;
    }
    for (const k of ['name', 'label', 'path']) str(t[k], `${what}.${k}`);
    optStr(t.iri, `${what}.iri`);
    for (const end of ['source', 'destination']) {
      const e = t[end];
      if (!obj(e) || typeof e.key !== 'string' || typeof e.references !== 'string') {
        out.push(`${what}.${end} is not { key, references }`);
      }
    }
    count(t.record_count, `${what}.record_count`);
    properties(t.properties, what);
  }
  return out;
}

const FIXTURES = ['../conformance/corpus/fossil.json'].map((path) => [
  path,
  readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8'),
]);

describe('the fixtures have the contract’s shape', () => {
  it.each(FIXTURES)('%s', (_, text) => {
    expect(shapeOf(JSON.parse(text))).toEqual([]);
  });

  it('and the check is not vacuous', () => {
    const broken = JSON.parse(FIXTURES[0]![1]!);
    delete broken.vertex_tables[0].key;
    broken.edge_tables[0].source = 'Person';
    expect(shapeOf(broken)).toEqual(['vertex_tables[0].key is not a string', 'edge_tables[0].source is not { key, references }']);
  });
});

describe('parseManifest', () => {
  it('reads fossil/1 and ignores what it does not know', () => {
    const m = parseManifest(JSON.stringify({ format: 'fossil/1', vertex_tables: [], edge_tables: [], later: 1 }), 'x');
    expect(m.vertex_tables).toEqual([]);
  });

  it.each([
    ['another format', JSON.stringify({ format: 'fossil/2' }), /fossil\/2/],
    ['no format', JSON.stringify({ vertex_tables: [] }), /undefined/],
    ['the old index', 'name: graph\nversion: gar/v1\n', /not JSON/],
    ['null', 'null', /undefined/],
  ])('refuses %s', (_, text, message) => {
    expect(() => parseManifest(text, 'fossil.json')).toThrow(CorpusManifestError);
    expect(() => parseManifest(text, 'fossil.json')).toThrow(message);
  });
});
