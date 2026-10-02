import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { parseManifest } from '../src/manifest.js';

/**
 * `fossil.json`: what the reader refuses, and the one shape the writer and the reader agree on.
 *
 * The shape is the writer's. `crates/fossil-sinks/fossil.schema.json` is generated from
 * `fossil_sinks::manifest::Manifest` (`FOSSIL_BLESS=1 cargo test -p fossil-sinks --test schema`),
 * and every fixture must validate against it. The reader exports no type of the manifest — a
 * caller reads it as `fossil_tables` and `fossil_columns` — so there is no second declaration of
 * the shape here to drift from the Rust.
 */

type Schema = Record<string, unknown>;
const SCHEMA = JSON.parse(
  readFileSync(fileURLToPath(new URL('../../../crates/fossil-sinks/fossil.schema.json', import.meta.url)), 'utf8'),
) as Schema;
const DEFINITIONS = SCHEMA.definitions as Record<string, Schema>;
const deref = (s: Schema): Schema =>
  typeof s.$ref === 'string' ? DEFINITIONS[s.$ref.replace('#/definitions/', '')]! : s;
/** A `$ref`, or an `allOf`/`anyOf` that is one `$ref` (plus `null`), is that definition. */
const refName = (s: Schema): string | undefined => {
  if (typeof s.$ref === 'string') return s.$ref.replace('#/definitions/', '');
  const arms = ((s.allOf ?? s.anyOf) as Schema[] | undefined)?.filter((a) => a.type !== 'null');
  return arms?.length === 1 ? refName(arms[0]!) : undefined;
};

/** The keywords `validate` understands. A schema that grows another fails here, not silently. */
const KNOWN = new Set([
  '$schema', 'title', 'description', 'type', 'required', 'properties', 'items', '$ref',
  'definitions', 'oneOf', 'anyOf', 'allOf', 'enum', 'format', 'minimum',
]);

/** Every way `v` departs from `s`, as `path: reason`. The subset of draft-07 the schema uses. */
function validate(v: unknown, s: Schema, path = '$'): string[] {
  const unknown = Object.keys(s).filter((k) => !KNOWN.has(k));
  if (unknown.length) return [`${path}: the schema uses ${unknown.join(', ')}, which this check does not read`];
  if (s.$ref) return validate(v, deref(s), path);
  if (s.allOf) return (s.allOf as Schema[]).flatMap((a) => validate(v, a, path));
  if (s.anyOf) return (s.anyOf as Schema[]).some((a) => !validate(v, a, path).length) ? [] : [`${path}: matches no anyOf arm`];
  if (s.oneOf) {
    const n = (s.oneOf as Schema[]).filter((a) => !validate(v, a, path).length).length;
    return n === 1 ? [] : [`${path}: matches ${n} oneOf arms`];
  }
  const kinds = s.type === undefined ? undefined : ([] as unknown[]).concat(s.type);
  const kindOf = (x: unknown) =>
    x === null ? 'null' : Array.isArray(x) ? 'array' : Number.isInteger(x) ? 'integer' : typeof x;
  if (kinds && !kinds.includes(kindOf(v)) && !(kinds.includes('number') && typeof v === 'number')) {
    return [`${path}: is ${kindOf(v)}, not ${kinds.join(' | ')}`];
  }
  if (s.enum && !(s.enum as unknown[]).includes(v)) return [`${path}: ${JSON.stringify(v)} is not one of ${JSON.stringify(s.enum)}`];
  if (typeof s.minimum === 'number' && typeof v === 'number' && v < s.minimum) return [`${path}: below ${s.minimum}`];
  const out: string[] = [];
  if (Array.isArray(v) && s.items) v.forEach((x, i) => out.push(...validate(x, s.items as Schema, `${path}[${i}]`)));
  if (kindOf(v) === 'object') {
    const o = v as Record<string, unknown>;
    for (const k of (s.required as string[] | undefined) ?? []) if (!(k in o)) out.push(`${path}.${k}: missing`);
    for (const [k, sub] of Object.entries((s.properties as Record<string, Schema>) ?? {})) {
      if (k in o) out.push(...validate(o[k], sub, `${path}.${k}`));
    }
  }
  return out;
}

const FIXTURES = ['../conformance/corpus/fossil.json'].map((path) => [
  path,
  readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8'),
]);

describe('the writer’s schema', () => {
  it.each(FIXTURES)('validates %s', (_, text) => {
    expect(validate(JSON.parse(text), SCHEMA)).toEqual([]);
  });

  it('and the validation is not vacuous', () => {
    const broken = JSON.parse(FIXTURES[0]![1]!);
    delete broken.vertex_tables[0].key;
    broken.edge_tables[0].source = 'Person';
    broken.vertex_tables[0].properties[0].role = 'key';
    expect(validate(broken, SCHEMA)).toEqual([
      '$.edge_tables[0].source: is string, not object',
      '$.vertex_tables[0].key: missing',
      '$.vertex_tables[0].properties[0].role: matches no anyOf arm',
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
