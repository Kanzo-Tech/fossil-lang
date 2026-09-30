import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import ts from 'typescript';
import { describe, expect, it } from 'vitest';

import { FOSSIL_FORMAT, parseManifest } from '../src/manifest.js';

/**
 * `fossil.json`: what the reader refuses, and the one shape the writer and the reader agree on.
 *
 * The shape is the writer's. `crates/fossil-sinks/fossil.schema.json` is generated from
 * `fossil_sinks::manifest::Manifest` (`FOSSIL_BLESS=1 cargo test -p fossil-sinks --test schema`),
 * and it is held here twice: every fixture must validate against it, and every interface in
 * `src/manifest.ts` must declare exactly the fields, optionality and types of the schema
 * definition of the same name. The TypeScript is written by hand and this is what stops it
 * drifting from the Rust.
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

/** What a schema property is, spelled the way the TypeScript checker prints the type. */
function spelled(s: Schema): string {
  const ref = refName(s);
  if (ref) return ref;
  if (s.oneOf) return (s.oneOf as Schema[]).map(spelled).join(' | ');
  if (s.enum) return (s.enum as unknown[]).map((e) => JSON.stringify(e)).join(' | ');
  const kinds = ([] as unknown[]).concat(s.type).filter((k) => k !== 'null');
  if (kinds.length !== 1) throw new Error(`no spelling for ${JSON.stringify(s)}`);
  const kind = kinds[0];
  if (kind === 'array') return `readonly ${spelled(s.items as Schema)}[]`;
  if (kind === 'integer' || kind === 'number') return 'number';
  return kind as string;
}

/** Each interface `src/manifest.ts` exports, as `{ field: 'type' }` with `?` on an optional one. */
function interfaces(): Record<string, Record<string, string>> {
  const file = fileURLToPath(new URL('../src/manifest.ts', import.meta.url));
  const program = ts.createProgram([file], { strict: true, target: ts.ScriptTarget.ES2022 });
  const checker = program.getTypeChecker();
  const out: Record<string, Record<string, string>> = {};
  ts.forEachChild(program.getSourceFile(file)!, (node) => {
    if (!ts.isInterfaceDeclaration(node)) return;
    const fields: Record<string, string> = {};
    for (const symbol of checker.getDeclaredTypeOfSymbol(checker.getSymbolAtLocation(node.name)!).getProperties()) {
      const optional = (symbol.flags & ts.SymbolFlags.Optional) !== 0;
      const type = checker.getNonNullableType(checker.getTypeOfSymbol(symbol));
      fields[symbol.name + (optional ? '?' : '')] = checker.typeToString(type);
    }
    out[node.name.text] = fields;
  });
  return out;
}

/** The same, read off the schema: a field is optional when it is not `required`. */
function definitions(): Record<string, Record<string, string>> {
  const out: Record<string, Record<string, string>> = {};
  const one = (s: Schema): Record<string, string> => {
    // `Position` is a oneOf of two objects differing only in `by`: one interface with a union.
    const arms = (s.oneOf as Schema[] | undefined) ?? [s];
    const fields: Record<string, string[]> = {};
    for (const arm of arms) {
      const required = new Set((arm.required as string[]) ?? []);
      for (const [k, sub] of Object.entries(arm.properties as Record<string, Schema>)) {
        (fields[k + (required.has(k) ? '' : '?')] ??= []).push(spelled(sub));
      }
    }
    return Object.fromEntries(Object.entries(fields).map(([k, v]) => [k, [...new Set(v)].join(' | ')]));
  };
  out.Manifest = one(SCHEMA);
  for (const [name, s] of Object.entries(DEFINITIONS)) out[name] = one(s);
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
    broken.vertex_tables[0].position.by = 'guess';
    expect(validate(broken, SCHEMA)).toEqual([
      '$.edge_tables[0].source: is string, not object',
      '$.vertex_tables[0].key: missing',
      '$.vertex_tables[0].position: matches no anyOf arm',
    ]);
  });

  it('is what src/manifest.ts declares, interface by interface', () => {
    const declared = interfaces();
    // The one sanctioned narrowing: the reader types `format` as the one value it accepts.
    expect(declared.Manifest!.format).toBe(JSON.stringify(FOSSIL_FORMAT));
    declared.Manifest!.format = 'string';
    const schema = definitions();
    expect(Object.keys(schema).sort()).toEqual(['EdgeTable', 'Endpoint', 'Manifest', 'Position', 'Property', 'VertexTable']);
    expect(declared).toEqual(schema);
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
