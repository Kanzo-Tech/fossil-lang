import { Parser, Store, type Term } from 'n3';

import type { Manifest } from '../src/manifest.js';

/**
 * **An R2RML processor for the subset `mapping` writes, over DuckDB** — the test's oracle, and not a
 * second implementation of `mapping`: it reads the Turtle as a graph and does what R2RML says each
 * term means, so a mapping that says something other than the corpus means produces other triples.
 * It knows `rr:tableName` and `rr:sqlQuery` logical tables, column-valued subject and object maps and
 * `rr:class` — and refuses anything else by name, so a mapping that grows a construct this does not
 * read fails here rather than reading as empty.
 *
 * `catalog` is where the corpus was attached: the one thing R2RML leaves to the processor's
 * connection, so every query runs with it as the default catalog.
 */

const RR = 'http://www.w3.org/ns/r2rml#';
const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';

/** The base the mapping's own nodes are read against. Any base does: they are never output. */
export const BASE = 'https://mapping.test/fossil';

/** The mapping as a graph, its relative nodes resolved against {@link BASE}. */
export function parse(turtle: string): Store {
  return new Store(new Parser({ baseIRI: BASE }).parse(turtle));
}

/** One triple, spelled as N-Triples spells it, so two sets compare as strings. */
export type Triple = string;

export const nt = {
  iri: (value: string): string => `<${value}>`,
  literal: (value: string, datatype: string): string => `${JSON.stringify(value)}^^<${datatype}>`,
};

/** A delimited SQL identifier, undelimited: the name a result row is keyed by. */
export function undelimit(identifier: string): string {
  if (!/^"(?:[^"]|"")*"$/.test(identifier)) throw new Error(`${identifier} is not a delimited identifier`);
  return identifier.slice(1, -1).replace(/""/g, '"');
}

export async function materialise(
  turtle: string,
  catalog: string,
  query: (sql: string) => Promise<Record<string, unknown>[]>,
): Promise<Set<Triple>> {
  const graph = parse(turtle);
  const all = (subject: Term, predicate: string): Term[] => graph.getObjects(subject, RR + predicate, null);
  const one = (subject: Term, predicate: string): Term => {
    const objects = all(subject, predicate);
    if (objects.length !== 1) throw new Error(`${subject.value} has ${objects.length} rr:${predicate}`);
    return objects[0]!;
  };
  const known = new Set(['logicalTable', 'subjectMap', 'predicateObjectMap', 'tableName', 'sqlQuery', 'sqlVersion',
    'column', 'termType', 'class', 'predicate', 'objectMap', 'datatype']);
  for (const quad of graph.getQuads(null, null, null, null)) {
    const p = quad.predicate.value;
    if (p.startsWith(RR) && !known.has(p.slice(RR.length))) throw new Error(`rr:${p.slice(RR.length)}, which this oracle does not read`);
  }

  /** A logical table as a `SELECT` (R2RML §5.2, its effective SQL query). */
  const relation = (table: Term): string => {
    const [name] = all(table, 'tableName');
    if (name !== undefined) return `SELECT * FROM ${name.value}`;
    if (one(table, 'sqlVersion').value !== `${RR}SQL2008`) throw new Error('an rr:sqlQuery not in Core SQL 2008');
    return one(table, 'sqlQuery').value;
  };

  /** A column-valued term map, as the term it makes of one row — or nothing for a null (§11). */
  const term = (map: Term, row: Record<string, unknown>, fallback: 'IRI' | 'Literal'): string | undefined => {
    const value = row[undelimit(one(map, 'column').value)];
    if (value === null || value === undefined) return undefined;
    const kind = all(map, 'termType')[0]?.value.slice(RR.length) ?? fallback;
    if (kind === 'IRI') return nt.iri(String(value));
    if (kind !== 'Literal') throw new Error(`term type ${kind}`);
    return nt.literal(String(value), one(map, 'datatype').value);
  };

  const triples = new Set<Triple>();
  await query(`USE "${catalog.replace(/"/g, '""')}"`);
  try {
    for (const map of graph.getSubjects(RDF_TYPE, `${RR}TriplesMap`, null)) {
      const subjectMap = one(map, 'subjectMap');
      const classes = all(subjectMap, 'class');
      const poms = all(map, 'predicateObjectMap').map((pom) => ({
        predicate: nt.iri(one(pom, 'predicate').value),
        object: one(pom, 'objectMap'),
      }));
      // Every value as text: RDF has no other lexical form, and DuckDB's cast is the column's.
      const rows = await query(`SELECT COLUMNS(*)::VARCHAR FROM (${relation(one(map, 'logicalTable'))})`);
      for (const row of rows) {
        const subject = term(subjectMap, row, 'IRI');
        if (subject === undefined) continue;
        for (const c of classes) triples.add(`${subject} <${RDF_TYPE}> ${nt.iri(c.value)}`);
        for (const { predicate, object } of poms) {
          const o = term(object, row, 'Literal');
          if (o !== undefined) triples.add(`${subject} ${predicate} ${o}`);
        }
      }
    }
  } finally {
    await query('USE memory');
  }
  return triples;
}

/**
 * **What the corpus holds as RDF**, written against the manifest and the views `open` made — SQL a
 * test states, not the mapping, so {@link materialise} is held to something it did not compute. The
 * caller states the datatype of each type word its corpus uses; a word it did not state is refused.
 */
export async function held(
  manifest: Manifest,
  catalog: string,
  query: (sql: string) => Promise<Record<string, unknown>[]>,
  typed: Readonly<Record<string, string>>,
): Promise<Set<Triple>> {
  const out = new Set<Triple>();
  for (const t of manifest.vertex_tables) {
    for (const r of await query(`SELECT subject FROM ${catalog}."${t.name}"`)) {
      if (t.iri) out.add(`${nt.iri(String(r.subject))} <${RDF_TYPE}> ${nt.iri(t.iri)}`);
    }
    for (const p of t.properties.filter((p) => p.iri !== undefined)) {
      const datatype = typed[p.type];
      if (datatype === undefined) throw new Error(`a ${p.type} column the test did not say the datatype of`);
      const rows = await query(`SELECT subject, "${p.name}"::VARCHAR AS v FROM ${catalog}."${t.name}" WHERE "${p.name}" IS NOT NULL`);
      for (const r of rows) out.add(`${nt.iri(String(r.subject))} ${nt.iri(p.iri!)} ${nt.literal(String(r.v), datatype)}`);
    }
  }
  for (const e of manifest.edge_tables.filter((e) => e.iri !== undefined)) {
    const rows = await query(
      `SELECT s.subject AS s, d.subject AS d FROM ${catalog}."${e.name}" e
         JOIN ${catalog}."${e.source.references}" s ON e.src = s.dense_id
         JOIN ${catalog}."${e.destination.references}" d ON e.dst = d.dense_id`,
    );
    for (const r of rows) out.add(`${nt.iri(String(r.s))} ${nt.iri(e.iri!)} ${nt.iri(String(r.d))}`);
  }
  return out;
}
