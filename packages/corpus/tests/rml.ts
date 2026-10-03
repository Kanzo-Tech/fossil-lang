import { Parser, Store, type Term } from 'n3';

import type { Manifest } from '../src/manifest.js';

/**
 * **An RML processor for the subset `mapping` writes, over DuckDB** — the test's oracle, and not a
 * second implementation of `mapping`: it reads the Turtle as a graph and does what the RML
 * specifications say each term means, so a mapping that says something other than the corpus means
 * produces other triples. It knows `rml:SQL2008Table` sources, `rml:LogicalView`s with expression
 * fields and inner joins, and reference-valued subject and object maps — and refuses anything else by
 * name, so a mapping that grows a construct this does not read fails here rather than reading as
 * empty.
 *
 * `catalog` is where the corpus was attached: the one thing RML leaves to the consumer, and the
 * `<#corpus>` source the mapping names.
 */

const RML = 'http://w3id.org/rml/';
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

export async function materialise(
  turtle: string,
  catalog: string,
  query: (sql: string) => Promise<Record<string, unknown>[]>,
): Promise<Set<Triple>> {
  const graph = parse(turtle);
  const one = (subject: Term, predicate: string): Term => {
    const objects = graph.getObjects(subject, RML + predicate, null);
    if (objects.length !== 1) throw new Error(`${subject.value} has ${objects.length} rml:${predicate}`);
    return objects[0]!;
  };
  const maybe = (subject: Term, predicate: string): Term | undefined => graph.getObjects(subject, RML + predicate, null)[0];
  const isA = (subject: Term, type: string): boolean => graph.getQuads(subject, RDF_TYPE, RML + type, null).length > 0;
  const q = (name: string): string => `"${name.replace(/"/g, '""')}"`;

  /** A logical source as a `SELECT`: its columns, or its fields, by name. */
  const relation = (source: Term): string => {
    if (isA(source, 'LogicalSource')) {
      expectValue(one(source, 'referenceFormulation'), `${RML}SQL2008Table`);
      // The iterator of an `rml:SQL2008Table` source is a table name, delimited, and RML-IO turns it
      // into `SELECT * FROM {table}` — under the catalog the consumer attached the corpus to.
      return `SELECT * FROM ${q(catalog)}.${one(source, 'iterator').value}`;
    }
    if (!isA(source, 'LogicalView')) throw new Error(`${source.value} is neither a logical source nor a view`);
    const fields = (owner: Term, from: string): string[] =>
      graph.getObjects(owner, `${RML}field`, null).map((f) => {
        if (!isA(f, 'ExpressionField')) throw new Error(`a field of ${owner.value} is not an expression field`);
        return `${from}.${q(one(f, 'reference').value)} AS ${q(one(f, 'fieldName').value)}`;
      });
    for (const unread of ['leftJoin', 'iterator']) {
      if (maybe(source, unread)) throw new Error(`${source.value} uses rml:${unread}, which this oracle does not read`);
    }
    let sql = `SELECT ${fields(source, 'b').join(', ')} FROM (${relation(one(source, 'viewOn'))}) b`;
    for (const [at, join] of graph.getObjects(source, `${RML}innerJoin`, null).entries()) {
      const condition = one(join, 'joinCondition');
      const p = `p${at}`;
      sql = `SELECT v.*, ${fields(join, p).join(', ')} FROM (${sql}) v JOIN (${relation(one(join, 'parentLogicalView'))}) ${p}
               ON v.${q(one(condition, 'child').value)} = ${p}.${q(one(condition, 'parent').value)}`;
    }
    return sql;
  };

  /** A reference-valued term map, as the term it makes of one row — or nothing for a null. */
  const term = (map: Term, row: Record<string, unknown>, fallback: 'IRI' | 'Literal'): string | undefined => {
    const value = row[one(map, 'reference').value];
    if (value === null || value === undefined) return undefined;
    const kind = maybe(map, 'termType')?.value.slice(RML.length) ?? fallback;
    if (kind === 'IRI') return nt.iri(String(value));
    if (kind !== 'Literal') throw new Error(`term type ${kind}`);
    return nt.literal(String(value), one(map, 'datatype').value);
  };

  const triples = new Set<Triple>();
  for (const map of graph.getSubjects(RDF_TYPE, `${RML}TriplesMap`, null)) {
    const subjectMap = one(map, 'subjectMap');
    const classes = graph.getObjects(subjectMap, `${RML}class`, null);
    const poms = graph.getObjects(map, `${RML}predicateObjectMap`, null).map((pom) => ({
      predicate: nt.iri(one(pom, 'predicate').value),
      object: one(pom, 'objectMap'),
    }));
    // Every value as text: RDF has no other lexical form, and DuckDB's cast is the column's.
    const select = `SELECT * FROM (${relation(one(map, 'logicalSource'))})`;
    const rows = await query(`SELECT COLUMNS(*)::VARCHAR FROM (${select})`);
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
  return triples;
}

function expectValue(term: Term, value: string): void {
  if (term.value !== value) throw new Error(`expected ${value}, found ${term.value}`);
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
