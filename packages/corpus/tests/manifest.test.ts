import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import Ajv from 'ajv';
import { describe, expect, it } from 'vitest';

import { parseManifest } from '../src/manifest.js';

/**
 * `fossil.json`: what the reader refuses, and the one shape the writer and the reader agree on.
 *
 * The shape is the writer's: `crates/fossil-sinks/fossil.schema.json`, derived from
 * `fossil_sinks::manifest::Manifest`, and every fixture validates against it.
 */
const SCHEMA = JSON.parse(
  readFileSync(fileURLToPath(new URL('../../../crates/fossil-sinks/fossil.schema.json', import.meta.url)), 'utf8'),
) as object;
const validate = new Ajv({ strict: true, allErrors: true, allowUnionTypes: true })
  .addFormat('uint64', { type: 'number', validate: (n: number) => Number.isSafeInteger(n) && n >= 0 })
  .compile(SCHEMA);
/** The instance paths `v` fails at. */
const errors = (v: unknown): string[] =>
  validate(v) ? [] : [...new Set((validate.errors ?? []).map((e) => e.instancePath))].sort();

const FIXTURES = ['../conformance/corpus/fossil.json'].map((path) => [
  path,
  readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8'),
]);

describe('the writer’s schema', () => {
  it.each(FIXTURES)('validates %s', (_, text) => {
    expect(errors(JSON.parse(text))).toEqual([]);
  });

  it('and the validation is not vacuous', () => {
    const broken = JSON.parse(FIXTURES[0]![1]!);
    delete broken.vertex_tables[0].key;
    broken.edge_tables[0].source = 'Person';
    broken.vertex_tables[0].properties[0].role = 'key';
    expect(errors(broken)).toEqual([
      '/edge_tables/0/source',
      '/vertex_tables/0',
      '/vertex_tables/0/properties/0/role',
    ]);
  });
});

describe('parseManifest', () => {
  it('reads fossil/1 and ignores what it does not know', () => {
    const m = parseManifest(JSON.stringify({ format: 'fossil/1', vertex_tables: [], edge_tables: [], later: 1 }), 'x');
    expect(m.vertex_tables).toEqual([]);
  });

  it.each([
    ['another format', JSON.stringify({ format: 'fossil/2' }), 'corpus/unsupported-format', { format: 'fossil/2' }],
    ['no format', JSON.stringify({ vertex_tables: [] }), 'corpus/unsupported-format', { format: 'undefined' }],
    ['the old index', 'name: graph\nversion: gar/v1\n', 'corpus/not-json', {}],
    ['null', 'null', 'corpus/unsupported-format', { format: 'undefined' }],
  ])('refuses %s', (_, text, code, data) => {
    expect(() => parseManifest(text, 'fossil.json')).toThrow(
      expect.objectContaining({ name: 'FossilError', code, data: { path: 'fossil.json', ...data } }),
    );
  });
});
