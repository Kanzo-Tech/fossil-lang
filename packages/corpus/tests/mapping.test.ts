import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Term } from 'n3';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { mapping, open } from '../src/index.js';
import { DATATYPES } from '../src/mapping.js';
import type { Manifest } from '../src/manifest.js';
import { duckdb } from './engine.js';
import { BASE, held, materialise, parse } from './rml.js';

/**
 * `mapping` — the corpus's RDF meaning, as RML. Three claims, each held against something that is not
 * the code under test:
 *
 * 1. **It is RML a parser reads**, and it maps exactly what the manifest gives an IRI — every type,
 *    column and relation that has one, nothing that has none — read back as a graph, not as text.
 * 2. **Its datatypes cover every type word the writer spells**, read out of the Rust that spells them.
 * 3. **It means the corpus**: the triples an RML processor makes of it over the checked-in corpus are
 *    the triples a second query, written here against the manifest, says the corpus holds.
 */

const RML = 'http://w3id.org/rml/';
const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const TEXT = readFileSync(join(CORPUS, 'fossil.json'), 'utf8');
const MANIFEST = JSON.parse(TEXT) as Manifest;
const node = (kind: string, name: string): string => `${BASE}#${kind}/${encodeURIComponent(name)}`;

describe('the mapping, as a graph', () => {
  const graph = parse(mapping(TEXT));
  const objects = (s: Term | string, p: string): Term[] => graph.getObjects(s, RML + p, null);
  const one = (s: Term | string, p: string): Term => {
    const all = objects(s, p);
    expect(all, `${typeof s === 'string' ? s : s.value} rml:${p}`).toHaveLength(1);
    return all[0]!;
  };
  /** The table name a logical source iterates, undelimited. */
  const table = (source: Term): string => {
    expect(one(source, 'referenceFormulation').value).toBe(`${RML}SQL2008Table`);
    expect(one(source, 'source').value).toBe(`${BASE}#corpus`);
    return JSON.parse(one(source, 'iterator').value) as string;
  };
  const fieldsOf = (owner: Term): Record<string, string> =>
    Object.fromEntries(objects(owner, 'field').map((f) => [one(f, 'fieldName').value, one(f, 'reference').value]));

  it('is Turtle, and every triples map is one the manifest asks for', () => {
    const maps = graph.getSubjects(RDF_TYPE, `${RML}TriplesMap`, null).map((m) => m.value).sort();
    const asked = [
      ...MANIFEST.vertex_tables.filter((t) => t.iri !== undefined || t.properties.some((p) => p.iri !== undefined)),
      ...MANIFEST.edge_tables.filter((e) => e.iri !== undefined),
    ];
    expect(maps).toEqual(asked.map((t) => node('map', t.name)).sort());
  });

  it.each(MANIFEST.vertex_tables.map((t) => [t.name, t] as const))('maps vertex type %s: its class, and each column with an IRI', (_, t) => {
    const map = node('map', t.name);
    expect(table(one(map, 'logicalSource'))).toBe(t.name);
    const subject = one(map, 'subjectMap');
    expect(one(subject, 'reference').value).toBe(t.identity);
    expect(one(subject, 'termType').value).toBe(`${RML}IRI`);
    expect(one(subject, 'class').value).toBe(t.iri);
    const poms = objects(map, 'predicateObjectMap').map((pom) => {
      const o = one(pom, 'objectMap');
      return [one(pom, 'predicate').value, one(o, 'reference').value, one(o, 'termType').value, one(o, 'datatype').value];
    });
    expect(poms).toEqual(
      t.properties.filter((p) => p.iri !== undefined).map((p) => [p.iri, p.name, `${RML}Literal`, DATATYPES[p.type]]),
    );
  });

  it.each(MANIFEST.edge_tables.map((t) => [t.name, t] as const))(
    'maps relation %s: src and dst joined to the dense_id of the tables they reference, subject to subject',
    (_, e) => {
      const map = node('map', e.name);
      const view = one(map, 'logicalSource');
      expect(graph.getQuads(view, RDF_TYPE, `${RML}LogicalView`, null)).toHaveLength(1);
      expect(table(one(view, 'viewOn'))).toBe(e.name);
      expect(fieldsOf(view)).toEqual({ [e.source.key]: e.source.key, [e.destination.key]: e.destination.key });

      const joins = objects(view, 'innerJoin').map((j) => {
        const parent = one(j, 'parentLogicalView');
        const condition = one(j, 'joinCondition');
        return {
          child: one(condition, 'child').value,
          parentTable: table(one(parent, 'viewOn')),
          parent: one(condition, 'parent').value,
          parentFields: fieldsOf(parent),
          fields: fieldsOf(j),
        };
      });
      const end = (key: string, references: string, as: string) => {
        const v = MANIFEST.vertex_tables.find((t) => t.name === references)!;
        return {
          child: key,
          parentTable: references,
          parent: v.key,
          parentFields: { [v.key]: v.key, [v.identity]: v.identity },
          fields: { [as]: v.identity },
        };
      };
      expect(joins).toEqual([
        end(e.source.key, e.source.references, 'source'),
        end(e.destination.key, e.destination.references, 'destination'),
      ]);

      const subject = one(map, 'subjectMap');
      expect([one(subject, 'reference').value, one(subject, 'termType').value]).toEqual(['source', `${RML}IRI`]);
      const [pom] = objects(map, 'predicateObjectMap');
      expect(one(pom!, 'predicate').value).toBe(e.iri);
      const object = one(pom!, 'objectMap');
      expect([one(object, 'reference').value, one(object, 'termType').value]).toEqual(['destination', `${RML}IRI`]);
    },
  );

  it('maps nothing that has no IRI, and says so by having nothing to say', () => {
    const bare = {
      ...MANIFEST,
      vertex_tables: MANIFEST.vertex_tables.map(({ iri: _, ...t }) => ({ ...t, properties: t.properties.map(({ iri: __, ...p }) => p) })),
      edge_tables: MANIFEST.edge_tables.map(({ iri: _, ...e }) => e),
    };
    const g = parse(mapping(JSON.stringify(bare)));
    expect(g.getSubjects(RDF_TYPE, `${RML}TriplesMap`, null)).toEqual([]);
    expect(g.getSubjects(RDF_TYPE, `${RML}LogicalSource`, null)).toEqual([]);

    // A type with no IRI keeps the columns that have one, and its subjects carry no class.
    const [person] = MANIFEST.vertex_tables;
    const classless = { ...MANIFEST, vertex_tables: [{ ...person!, iri: undefined }], edge_tables: [] };
    const c = parse(mapping(JSON.stringify(classless)));
    const subject = c.getObjects(node('map', person!.name), `${RML}subjectMap`, null)[0]!;
    expect(c.getObjects(subject, `${RML}class`, null)).toEqual([]);
    expect(c.getObjects(node('map', person!.name), `${RML}predicateObjectMap`, null)).toHaveLength(
      person!.properties.filter((p) => p.iri !== undefined).length,
    );
  });

  it('leaves a list column out: one cell would be several literals', () => {
    const [person] = MANIFEST.vertex_tables;
    const tags = { name: 'tags', type: 'list<string>', iri: 'https://example.org/tag', nullable: true };
    const g = parse(mapping(JSON.stringify({ ...MANIFEST, vertex_tables: [{ ...person!, properties: [...person!.properties, tags] }] })));
    expect(g.getSubjects(`${RML}predicate`, tags.iri, null)).toEqual([]);
  });

  it('delimits a table name in its iterator, and escapes what Turtle and SQL would not take', () => {
    const [person] = MANIFEST.vertex_tables;
    const odd = { ...person!, name: 'A "quoted" type' };
    const g = parse(mapping(JSON.stringify({ ...MANIFEST, vertex_tables: [odd], edge_tables: [] })));
    const source = g.getObjects(node('map', odd.name), `${RML}logicalSource`, null)[0]!;
    expect(g.getObjects(source, `${RML}iterator`, null)[0]!.value).toBe('"A ""quoted"" type"');
  });

  it('refuses a format it does not read, as open does', () => {
    expect(() => mapping(JSON.stringify({ ...MANIFEST, format: 'fossil/2' }))).toThrow(
      expect.objectContaining({ code: 'corpus/unsupported-format' }),
    );
  });
});

describe('the datatypes', () => {
  /** The type words `data_type_name` writes, read out of the Rust — the one place they are spelled. */
  const spelled = (): string[] => {
    const rust = readFileSync(fileURLToPath(new URL('../../../crates/fossil-sinks/src/manifest.rs', import.meta.url)), 'utf8');
    const body = rust.slice(rust.indexOf('pub fn data_type_name'), rust.indexOf('#[cfg(test)]'));
    return [...body.matchAll(/"([a-z0-9]+)"\.to_string\(\)/g)].map((m) => m[1]!);
  };

  it('cover every scalar type word the writer spells, and no other', () => {
    const words = spelled();
    expect(words.length).toBeGreaterThan(10);
    expect(Object.keys(DATATYPES).sort()).toEqual([...new Set(words)].sort());
  });

  it('are the canonical IRIs of the lattice: one integer, one float', () => {
    expect(new Set(['int8', 'int64', 'uint32'].map((w) => DATATYPES[w]))).toEqual(new Set(['http://www.w3.org/2001/XMLSchema#integer']));
    expect(new Set(['float', 'double'].map((w) => DATATYPES[w]))).toEqual(new Set(['http://www.w3.org/2001/XMLSchema#double']));
  });
});

describe('the mapping, executed', () => {
  let query: (sql: string) => Promise<Record<string, unknown>[]>;
  let close: () => Promise<void>;
  const scratch = mkdtempSync(join(tmpdir(), 'fossil-mapping-test-'));

  beforeAll(async () => {
    const db = await duckdb(join(scratch, 'spill'));
    query = db.query;
    close = await open('rdf', { engine: db.engine, url: CORPUS });
  }, 60_000);
  afterAll(async () => {
    await close?.();
    rmSync(scratch, { recursive: true, force: true });
  });

  it('makes the triples the corpus holds — no more, no fewer', async () => {
    const got = await materialise(mapping(TEXT), 'rdf', query);
    const want = await held(MANIFEST, 'rdf', query, {
      int32: `${XSD}integer`,
      string: `${XSD}string`,
      double: `${XSD}double`,
    });
    expect(want.size).toBeGreaterThan(1000);
    expect([...got].filter((t) => !want.has(t))).toEqual([]);
    expect([...want].filter((t) => !got.has(t))).toEqual([]);

    // And the edges are there by identity: every `knows` triple joins two Person subjects.
    const knows = MANIFEST.edge_tables.find((e) => e.name === 'Person_knows_Person')!;
    const [pairs] = await query(`SELECT count(*)::INTEGER AS n FROM (SELECT DISTINCT src, dst FROM rdf."${knows.name}")`);
    expect([...got].filter((t) => t.includes(` <${knows.iri}> `))).toHaveLength(pairs!.n as number);
  }, 120_000);
});
