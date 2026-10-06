/**
 * The corpus's RDF meaning, in R2RML: a pure function of `fossil.json` that answers a W3C R2RML
 * mapping (https://www.w3.org/TR/r2rml/) in Turtle.
 *
 * The meaning was always in the manifest: a vertex table has a class IRI and an identity column, a
 * column may carry a predicate IRI, an edge table has a predicate IRI and the two vertex tables its
 * endpoints reference. This states it once, in the W3C Recommendation for relational data as RDF, so
 * that a SHACL engine, a triplestore loader or any R2RML processor reads the corpus as RDF with no
 * fossil code. `/docs/format/reading/rdf` is the specification of what comes out;
 * `/docs/design/discarded` has RML 2.0, a file in `fossil.json`, a Rust generator and a
 * consumer-side adapter, and why none of them.
 */

import type { EdgeTable, Manifest, Property, PropertyTable, VertexTable } from './manifest.gen.js';
import { parseManifest } from './manifest.js';
import { ident } from './sql.js';

/**
 * **The corpus's RDF meaning, as an R2RML mapping in Turtle.** `manifest` is the text of `fossil.json`.
 *
 * - A vertex type is a `rr:TriplesMap` over its table (`rr:tableName`): the subject is the
 *   `identity` column as an IRI, of the type's class, and each column with a predicate IRI is a
 *   predicate-object map whose object is the column, with the `term_type` and `datatype` the shape
 *   declared copied as `rr:termType` and `rr:datatype`. Where it declared none, R2RML's default
 *   holds: a literal of the column's natural datatype (§10.2), which the processor derives.
 * - A relation is a `rr:TriplesMap` over an `rr:sqlQuery` in Core SQL 2008 (`rr:SQL2008`): the edge
 *   table inner-joined at each end to the vertex table its key references, answering the two
 *   identities as `source` and `destination` — the source's subject, the relation's predicate, the
 *   destination's subject as an IRI.
 * - A multi-valued property is the same query over its own table, a row per value (first normal
 *   form): joined at `src` to its vertex table, answering the source's identity and the `value`,
 *   whose object map is the value column's, as a vertex column's is.
 * - A type, column, relation or property without an IRI is not mapped; a type that would map to
 *   nothing has no triples map.
 * - Every table and column is a delimited identifier, unqualified: the consumer resolves the names
 *   against the database it attached the corpus to.
 *
 * It reads nothing and derives nothing from the bytes: the same manifest answers the same text.
 *
 * @throws {FossilError} `corpus/not-json`, or `corpus/unsupported-format` for a format other than
 *   `fossil/1` — what `open` refuses, refused the same way.
 */
export function mapping(manifest: string): string {
  return render(parseManifest(manifest, 'fossil.json'));
}

const has = (iri: string | undefined): iri is string => iri !== undefined && iri !== '';

/** The predicate-mapped columns of a table: the program's, with an IRI. */
const mapped = (table: VertexTable | PropertyTable): Property[] =>
  table.properties.filter((p) => p.role === undefined && has(p.iri));

function render(manifest: Manifest): string {
  const vertices = new Map(manifest.vertex_tables.map((t) => [t.name, t]));
  const maps = [
    ...manifest.vertex_tables.filter((t) => has(t.iri) || mapped(t).length > 0).map(vertexMap),
    // A relation naming a type with no table has no subject to take at that end, so it is not mapped.
    ...manifest.edge_tables
      .filter((e) => has(e.iri) && vertices.has(e.source.references) && vertices.has(e.destination.references))
      .map((e) => edgeMap(e, vertices.get(e.source.references)!, vertices.get(e.destination.references)!)),
    ...(manifest.property_tables ?? [])
      .filter((t) => mapped(t).length > 0 && vertices.has(t.source.references))
      .map((t) => valuesMap(t, vertices.get(t.source.references)!)),
  ];
  return `${[HEADER, ...maps].join('\n\n')}\n`;
}

const HEADER = [
  '# The RDF meaning of a fossil/1 corpus, in R2RML (https://www.w3.org/TR/r2rml/).',
  '# Every table and column is a delimited identifier, unqualified: resolve the names against the',
  '# database the corpus is attached to.',
  '@prefix rr: <http://www.w3.org/ns/r2rml#> .',
].join('\n');

/** A vertex type: its identity as the subject, its class, and a literal per mapped column. */
function vertexMap(table: VertexTable): string {
  const subject = [`rr:column ${column(table.identity)}`, 'rr:termType rr:IRI'];
  if (has(table.iri)) subject.push(`rr:class ${iri(table.iri)}`);
  const lines = [
    `${node(table.name)} a rr:TriplesMap ;`,
    `  rr:logicalTable [ rr:tableName ${str(ident(table.name))} ] ;`,
    `  rr:subjectMap [ ${subject.join(' ; ')} ]`,
  ];
  for (const p of mapped(table)) {
    lines[lines.length - 1] += ' ;';
    lines.push(
      `  rr:predicateObjectMap [ rr:predicate ${iri(p.iri!)} ; rr:objectMap [ ` +
        `${objectMap(p)} ] ]`,
    );
  }
  return `${lines.join('\n')} .`;
}

/**
 * A relation: the edge table joined at each end to the vertex table its key references — `src` to the
 * source's `dense_id`, `dst` to the destination's — answering the two subjects, and a triples map over
 * it. The query's only names are the manifest's, delimited, and its two output columns are fixed, so
 * no name in it can collide with another.
 */
function edgeMap(edge: EdgeTable, from: VertexTable, to: VertexTable): string {
  return joinedMap(
    edge.name,
    [`s.${ident(from.identity)} AS "source"`, `d.${ident(to.identity)} AS "destination"`],
    [
      `JOIN ${ident(from.name)} AS s ON e.${ident(edge.source.key)} = s.${ident(from.key)}`,
      `JOIN ${ident(to.name)} AS d ON e.${ident(edge.destination.key)} = d.${ident(to.key)}`,
    ],
    edge.iri!,
    `rr:column ${column('destination')} ; rr:termType rr:IRI`,
  );
}

/**
 * A multi-valued property: its table joined at `src` to the vertex table it references, answering the
 * vertex's subject and the value, and a triples map over it whose object is the value, as the column
 * declares it.
 */
function valuesMap(table: PropertyTable, from: VertexTable): string {
  const value = mapped(table)[0]!;
  return joinedMap(
    table.name,
    [`s.${ident(from.identity)} AS "source"`, `e.${ident(value.name)} AS "value"`],
    [`JOIN ${ident(from.name)} AS s ON e.${ident(table.source.key)} = s.${ident(from.key)}`],
    value.iri!,
    objectMap({ ...value, name: 'value' }),
  );
}

/** A triples map over a query on `table`, `e`: its columns, its joins, and one predicate-object map. */
function joinedMap(table: string, select: string[], joins: string[], predicate: string, object: string): string {
  const query = [`SELECT ${select.join(', ')}`, `FROM ${ident(table)} AS e`, ...joins].join('\n');
  return [
    `${node(table)} a rr:TriplesMap ;`,
    `  rr:logicalTable [ rr:sqlQuery ${str(query)} ; rr:sqlVersion rr:SQL2008 ] ;`,
    `  rr:subjectMap [ rr:column ${column('source')} ; rr:termType rr:IRI ] ;`,
    `  rr:predicateObjectMap [ rr:predicate ${iri(predicate)} ; rr:objectMap [ ${object} ] ] .`,
  ].join('\n');
}

/** A column name as R2RML reads it: a delimited identifier, case kept (R2RML §6, §10.1). */
const column = (name: string): string => str(ident(name));

/** A column's object map: the column, and the term the shape declared for it, verbatim. */
const objectMap = (p: Property): string =>
  [
    `rr:column ${column(p.name)}`,
    ...(has(p.term_type) ? [`rr:termType ${iri(p.term_type)}`] : []),
    ...(has(p.datatype) ? [`rr:datatype ${iri(p.datatype)}`] : []),
  ].join(' ; ');

/**
 * A triples map, relative to wherever the document is kept. The name is percent-encoded, so no table
 * can collide with another.
 */
const node = (name: string): string => `<#map/${encodeURIComponent(name)}>`;

/** An IRI as a Turtle `IRIREF`; a character an `IRIREF` cannot hold is written as a `UCHAR`. */
const iri = (value: string): string =>
  `<${value.replace(/[\u0000- <>"{}|^`\\]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`)}>`;

/** A Turtle string literal. */
const str = (value: string): string =>
  `"${value.replace(/[\\"\n\r]/g, (c) => ({ '\\': '\\\\', '"': '\\"', '\n': '\\n', '\r': '\\r' })[c]!)}"`;
