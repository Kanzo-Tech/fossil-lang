import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Term } from 'n3';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { mapping, open } from '../src/index.js';
import type { Manifest } from '../src/manifest.js';
import { duckdb } from './engine.js';
import { BASE, held, materialise, parse, undelimit } from './r2rml.js';

/**
 * `mapping` — the corpus's RDF meaning, as R2RML. Three claims, each held against something that is
 * not the code under test:
 *
 * 1. **It is R2RML a parser reads**, and it maps exactly what the manifest gives an IRI — every type,
 *    column and relation that has one, nothing that has none — read back as a graph, not as text.
 * 2. **It states the term the shape declared, verbatim**, and none where the shape declared none —
 *    R2RML's natural datatype (§10.2) is then the processor's to derive, not the mapping's.
 * 3. **It means the corpus**: the triples an R2RML processor makes of it over the checked-in corpus
 *    are the triples a second query, written here against the manifest, says the corpus holds.
 */

const RR = 'http://www.w3.org/ns/r2rml#';
const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const CORPUS = fileURLToPath(new URL('../conformance/corpus', import.meta.url));
const TEXT = readFileSync(join(CORPUS, 'fossil.json'), 'utf8');
const MANIFEST = JSON.parse(TEXT) as Manifest;
const node = (name: string): string => `${BASE}#map/${encodeURIComponent(name)}`;
const q = (name: string): string => `"${name.replace(/"/g, '""')}"`;

describe('the mapping, as a graph', () => {
  const graph = parse(mapping(TEXT));
  const objects = (s: Term | string, p: string): Term[] => graph.getObjects(s, RR + p, null);
  const one = (s: Term | string, p: string): Term => {
    const all = objects(s, p);
    expect(all, `${typeof s === 'string' ? s : s.value} rr:${p}`).toHaveLength(1);
    return all[0]!;
  };
  /** The column a term map reads, undelimited. */
  const column = (map: Term): string => undelimit(one(map, 'column').value);

  it('is Turtle, and every triples map is one the manifest asks for', () => {
    const maps = graph.getSubjects(RDF_TYPE, `${RR}TriplesMap`, null).map((m) => m.value).sort();
    const asked = [
      ...MANIFEST.vertex_tables.filter((t) => t.iri !== undefined || t.properties.some((p) => p.iri !== undefined)),
      ...MANIFEST.edge_tables.filter((e) => e.iri !== undefined),
    ];
    expect(maps).toEqual(asked.map((t) => node(t.name)).sort());
  });

  it.each(MANIFEST.vertex_tables.map((t) => [t.name, t] as const))('maps vertex type %s: its class, and each column with an IRI', (_, t) => {
    const map = node(t.name);
    expect(undelimit(one(one(map, 'logicalTable'), 'tableName').value)).toBe(t.name);
    const subject = one(map, 'subjectMap');
    expect(column(subject)).toBe(t.identity);
    expect(one(subject, 'termType').value).toBe(`${RR}IRI`);
    expect(one(subject, 'class').value).toBe(t.iri);
    const poms = objects(map, 'predicateObjectMap').map((pom) => {
      const o = one(pom, 'objectMap');
      return [one(pom, 'predicate').value, column(o), objects(o, 'termType')[0]?.value, objects(o, 'datatype')[0]?.value];
    });
    expect(poms).toEqual(
      t.properties.filter((p) => p.iri !== undefined).map((p) => [p.iri, p.name, p.term_type, p.datatype]),
    );
  });

  it.each(MANIFEST.edge_tables.map((t) => [t.name, t] as const))(
    'maps relation %s: src and dst joined to the dense_id of the tables they reference, subject to subject',
    (_, e) => {
      const map = node(e.name);
      const table = one(map, 'logicalTable');
      expect(one(table, 'sqlVersion').value).toBe(`${RR}SQL2008`);
      const [from, to] = [e.source.references, e.destination.references].map((n) => MANIFEST.vertex_tables.find((t) => t.name === n)!);
      expect(one(table, 'sqlQuery').value).toBe(
        [
          `SELECT s.${q(from!.identity)} AS "source", d.${q(to!.identity)} AS "destination"`,
          `FROM ${q(e.name)} AS e`,
          `JOIN ${q(from!.name)} AS s ON e.${q(e.source.key)} = s.${q(from!.key)}`,
          `JOIN ${q(to!.name)} AS d ON e.${q(e.destination.key)} = d.${q(to!.key)}`,
        ].join('\n'),
      );

      const subject = one(map, 'subjectMap');
      expect([column(subject), one(subject, 'termType').value]).toEqual(['source', `${RR}IRI`]);
      const [pom] = objects(map, 'predicateObjectMap');
      expect(one(pom!, 'predicate').value).toBe(e.iri);
      const object = one(pom!, 'objectMap');
      expect([column(object), one(object, 'termType').value]).toEqual(['destination', `${RR}IRI`]);
    },
  );

  it('maps nothing that has no IRI, and says so by having nothing to say', () => {
    const bare = {
      ...MANIFEST,
      vertex_tables: MANIFEST.vertex_tables.map(({ iri: _, ...t }) => ({ ...t, properties: t.properties.map(({ iri: __, ...p }) => p) })),
      edge_tables: MANIFEST.edge_tables.map(({ iri: _, ...e }) => e),
    };
    const g = parse(mapping(JSON.stringify(bare)));
    expect(g.getSubjects(RDF_TYPE, `${RR}TriplesMap`, null)).toEqual([]);

    // A type with no IRI keeps the columns that have one, and its subjects carry no class.
    const [person] = MANIFEST.vertex_tables;
    const classless = { ...MANIFEST, vertex_tables: [{ ...person!, iri: undefined }], edge_tables: [] };
    const c = parse(mapping(JSON.stringify(classless)));
    const subject = c.getObjects(node(person!.name), `${RR}subjectMap`, null)[0]!;
    expect(c.getObjects(subject, `${RR}class`, null)).toEqual([]);
    expect(c.getObjects(node(person!.name), `${RR}predicateObjectMap`, null)).toHaveLength(
      person!.properties.filter((p) => p.iri !== undefined).length,
    );
  });

  it('leaves a list column out: one cell would be several literals', () => {
    const [person] = MANIFEST.vertex_tables;
    const tags = { name: 'tags', type: 'list<string>', iri: 'https://example.org/tag', nullable: true };
    const g = parse(mapping(JSON.stringify({ ...MANIFEST, vertex_tables: [{ ...person!, properties: [...person!.properties, tags] }] })));
    expect(g.getSubjects(`${RR}predicate`, tags.iri, null)).toEqual([]);
  });

  it('delimits every table and column name, and escapes what Turtle and SQL would not take', () => {
    const [person] = MANIFEST.vertex_tables;
    const odd = { ...person!, name: 'A "quoted" type', properties: [{ name: 'birthYear', type: 'int32', iri: 'https://example.org/born' }] };
    const g = parse(mapping(JSON.stringify({ ...MANIFEST, vertex_tables: [odd], edge_tables: [] })));
    const map = node(odd.name);
    const table = g.getObjects(map, `${RR}logicalTable`, null)[0]!;
    expect(g.getObjects(table, `${RR}tableName`, null)[0]!.value).toBe('"A ""quoted"" type"');
    const [pom] = g.getObjects(map, `${RR}predicateObjectMap`, null);
    const object = g.getObjects(pom!, `${RR}objectMap`, null)[0]!;
    // Delimited, so case is kept: an R2RML processor does not fold `birthYear` to upper case.
    expect(g.getObjects(object, `${RR}column`, null)[0]!.value).toBe('"birthYear"');
  });

  it('copies the term the shape declared, and invents none', () => {
    const [person] = MANIFEST.vertex_tables;
    const declared = {
      ...person!,
      properties: [
        { name: 'born', type: 'int32', iri: 'https://example.org/born', term_type: `${RR}Literal`, datatype: `${XSD}gYear` },
        { name: 'homepage', type: 'string', iri: 'https://example.org/homepage', term_type: `${RR}IRI` },
        { name: 'postcode', type: 'string', iri: 'https://example.org/postcode' },
      ],
    };
    const g = parse(mapping(JSON.stringify({ ...MANIFEST, vertex_tables: [declared], edge_tables: [] })));
    const term = (predicate: string) => {
      const pom = g.getSubjects(`${RR}predicate`, predicate, null)[0]!;
      const [o] = g.getObjects(pom, `${RR}objectMap`, null);
      return ['termType', 'datatype'].map((p) => g.getObjects(o!, RR + p, null).map((t) => t.value));
    };
    expect(term('https://example.org/born')).toEqual([[`${RR}Literal`], [`${XSD}gYear`]]);
    expect(term('https://example.org/homepage')).toEqual([[`${RR}IRI`], []]);
    expect(term('https://example.org/postcode')).toEqual([[], []]);
  });

  it('refuses a format it does not read, as open does', () => {
    expect(() => mapping(JSON.stringify({ ...MANIFEST, format: 'fossil/2' }))).toThrow(
      expect.objectContaining({ code: 'corpus/unsupported-format' }),
    );
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

describe('the mapping, executed over names a processor could fold', () => {
  // A camelCase column, a table name that holds a `"`, and an edge between them: undelimited, an
  // R2RML processor would fold `birthYear` to `BIRTHYEAR` and find no column (#22).
  const ODD = 'A "quoted" type';
  const manifest = {
    format: 'fossil/1',
    vertex_tables: [
      {
        name: 'Person', iri: 'https://example.org/Person', path: 'p', key: 'dense_id', identity: 'subject', record_count: 2,
        properties: [
          { name: 'birthYear', type: 'int32', iri: 'https://example.org/birthYear', term_type: `${RR}Literal`, datatype: `${XSD}gYear` },
          { name: 'Homepage', type: 'string', iri: 'https://example.org/homepage', term_type: `${RR}IRI` },
          { name: 'Rank', type: 'int64', iri: 'https://example.org/rank' },
        ],
      },
      { name: ODD, iri: 'https://example.org/Odd', path: 'o', key: 'dense_id', identity: 'subject', record_count: 1, properties: [] },
    ],
    edge_tables: [
      {
        name: 'Person_likes_Odd', iri: 'https://example.org/likes', path: 'e', record_count: 1, properties: [],
        source: { key: 'src', references: 'Person' }, destination: { key: 'dst', references: ODD },
      },
    ],
  };
  let query: (sql: string) => Promise<Record<string, unknown>[]>;

  beforeAll(async () => {
    ({ query } = await duckdb());
    for (const statement of [
      `ATTACH ':memory:' AS odd`,
      `CREATE TABLE odd."Person" (dense_id UINTEGER, subject VARCHAR, "birthYear" INTEGER, "Homepage" VARCHAR, "Rank" BIGINT)`,
      `INSERT INTO odd."Person" VALUES (0, 'https://example.org/ada', 1815, 'https://ada.example/', 1), (1, 'https://example.org/alan', NULL, NULL, NULL)`,
      `CREATE TABLE odd."A ""quoted"" type" (dense_id UINTEGER, subject VARCHAR)`,
      `INSERT INTO odd."A ""quoted"" type" VALUES (2, 'https://example.org/engine')`,
      `CREATE TABLE odd."Person_likes_Odd" (src UINTEGER, dst UINTEGER)`,
      `INSERT INTO odd."Person_likes_Odd" VALUES (0, 2)`,
    ]) await query(statement);
  }, 60_000);

  it('reads every name as written, case and quotes kept, and makes the term each column declares', async () => {
    const got = await materialise(mapping(JSON.stringify(manifest)), 'odd', query);
    const type = (s: string, c: string) => `<https://example.org/${s}> <${RDF_TYPE}> <https://example.org/${c}>`;
    expect([...got].sort()).toEqual(
      [
        type('ada', 'Person'),
        type('alan', 'Person'),
        type('engine', 'Odd'),
        `<https://example.org/ada> <https://example.org/birthYear> "1815"^^<${XSD}gYear>`,
        '<https://example.org/ada> <https://example.org/homepage> <https://ada.example/>',
        // No term declared: R2RML's natural datatype of a BIGINT, which the processor derives.
        `<https://example.org/ada> <https://example.org/rank> "1"^^<${XSD}integer>`,
        '<https://example.org/ada> <https://example.org/likes> <https://example.org/engine>',
      ].sort(),
    );
  }, 60_000);
});
