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

import { parseManifest, type EdgeTable, type Manifest, type Property, type VertexTable } from './manifest.js';
import { ident } from './sql.js';

const XSD = 'http://www.w3.org/2001/XMLSchema#';

/**
 * The literal datatype of a column, by the manifest's type word — `corpus.bnf`'s spellings, which
 * `crates/fossil-sinks/src/manifest.rs, data_type_name` writes. It is R2RML's natural mapping of SQL
 * types (§10.2): every integer width is `xsd:integer` and every float `xsd:double`. A `list<…>` has
 * no entry: one cell would be several literals, which no term map produces, so a list column is not
 * mapped.
 *
 * `tests/mapping.test.ts` holds the keys to the spellings `data_type_name` writes, read out of the
 * Rust.
 */
export const DATATYPES: Readonly<Record<string, string>> = {
  string: `${XSD}string`,
  bool: `${XSD}boolean`,
  int8: `${XSD}integer`,
  int16: `${XSD}integer`,
  int32: `${XSD}integer`,
  int64: `${XSD}integer`,
  uint8: `${XSD}integer`,
  uint16: `${XSD}integer`,
  uint32: `${XSD}integer`,
  uint64: `${XSD}integer`,
  float: `${XSD}double`,
  double: `${XSD}double`,
  decimal: `${XSD}decimal`,
  date: `${XSD}date`,
  time: `${XSD}time`,
  timestamp: `${XSD}dateTime`,
  binary: `${XSD}hexBinary`,
};

/**
 * **The corpus's RDF meaning, as an R2RML mapping in Turtle.** `manifest` is the text of `fossil.json`.
 *
 * - A vertex type is a `rr:TriplesMap` over its table (`rr:tableName`): the subject is the
 *   `identity` column as an IRI, of the type's class, and each column with a predicate IRI is a
 *   predicate-object map whose object is the column as a literal of {@link DATATYPES}' datatype.
 * - A relation is a `rr:TriplesMap` over an `rr:sqlQuery` in Core SQL 2008 (`rr:SQL2008`): the edge
 *   table inner-joined at each end to the vertex table its key references, answering the two
 *   identities as `source` and `destination` — the source's subject, the relation's predicate, the
 *   destination's subject as an IRI.
 * - A type, column or relation without an IRI is not mapped; a type that would map to nothing has no
 *   triples map. A `list<…>` column is not mapped.
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

/** The predicate-mapped columns of a vertex table: an IRI, and a datatype a literal can carry. */
const mapped = (table: VertexTable): Property[] =>
  table.properties.filter((p) => has(p.iri) && DATATYPES[p.type] !== undefined);

function render(manifest: Manifest): string {
  const vertices = new Map(manifest.vertex_tables.map((t) => [t.name, t]));
  const maps = [
    ...manifest.vertex_tables.filter((t) => has(t.iri) || mapped(t).length > 0).map(vertexMap),
    // A relation naming a type with no table has no subject to take at that end, so it is not mapped.
    ...manifest.edge_tables
      .filter((e) => has(e.iri) && vertices.has(e.source.references) && vertices.has(e.destination.references))
      .map((e) => edgeMap(e, vertices.get(e.source.references)!, vertices.get(e.destination.references)!)),
  ];
  return `${[HEADER, ...maps].join('\n\n')}\n`;
}

const HEADER = [
  '# The RDF meaning of a fossil/1 corpus, in R2RML (https://www.w3.org/TR/r2rml/).',
  '# Every table and column is a delimited identifier, unqualified: resolve the names against the',
  '# database the corpus is attached to.',
  '@prefix rr: <http://www.w3.org/ns/r2rml#> .',
  '@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .',
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
        `rr:column ${column(p.name)} ; rr:termType rr:Literal ; rr:datatype ${datatype(p.type)} ] ]`,
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
  const query = [
    `SELECT s.${ident(from.identity)} AS "source", d.${ident(to.identity)} AS "destination"`,
    `FROM ${ident(edge.name)} AS e`,
    `JOIN ${ident(from.name)} AS s ON e.${ident(edge.source.key)} = s.${ident(from.key)}`,
    `JOIN ${ident(to.name)} AS d ON e.${ident(edge.destination.key)} = d.${ident(to.key)}`,
  ].join('\n');
  return [
    `${node(edge.name)} a rr:TriplesMap ;`,
    `  rr:logicalTable [ rr:sqlQuery ${str(query)} ; rr:sqlVersion rr:SQL2008 ] ;`,
    `  rr:subjectMap [ rr:column ${column('source')} ; rr:termType rr:IRI ] ;`,
    `  rr:predicateObjectMap [ rr:predicate ${iri(edge.iri!)} ; rr:objectMap [ ` +
      `rr:column ${column('destination')} ; rr:termType rr:IRI ] ] .`,
  ].join('\n');
}

/** A column name as R2RML reads it: a delimited identifier, case kept (R2RML §6, §10.1). */
const column = (name: string): string => str(ident(name));

/** A literal datatype as Turtle: the `xsd:` prefix where the IRI is in it. */
const datatype = (type: string): string => DATATYPES[type]!.replace(XSD, 'xsd:');

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
