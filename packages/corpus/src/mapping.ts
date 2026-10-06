/**
 * The corpus's RDF meaning, in RML: a pure function of `fossil.json` that answers an RML 2.0 mapping
 * — RML-Core, RML-IO and RML-LV — in Turtle.
 *
 * The meaning was always in the manifest: a vertex table has a class IRI and an identity column, a
 * column may carry a predicate IRI, an edge table has a predicate IRI and the two vertex tables its
 * endpoints reference. This states it once, in the W3C KG-Construct vocabulary, so that a SHACL
 * engine, a triplestore loader or any RML processor reads the corpus as RDF with no fossil code.
 * `/docs/format/reading/rdf` is the specification of what comes out; `/docs/design/discarded` has
 * R2RML, a file in `fossil.json`, a Rust generator and a consumer-side adapter, and why none of them.
 */

import type { EdgeTable, Manifest, Property, VertexTable } from './manifest.gen.js';
import { parseManifest } from './manifest.js';
import { ident } from './sql.js';

const XSD = 'http://www.w3.org/2001/XMLSchema#';

/**
 * The literal datatype of a column, by the manifest's type word — `corpus.bnf`'s spellings, which
 * `crates/fossil-sinks/src/manifest.rs, data_type_name` writes. It is R2RML's natural mapping of SQL
 * types, which is also the canonical IRI `crates/fossil-graph-schema/src/lib.rs, Primitive` gives
 * each primitive: every integer width is `xsd:integer` and every float `xsd:double`, so a shape
 * written `xsd:integer` matches a column stored `int32`. A `list<…>` has no entry: one cell would be
 * several literals, which no term map produces, so a list column is not mapped.
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

/** The names a view gives the two subjects an edge joins to: the manifest's own words for its ends. */
const ENDS = ['source', 'destination'] as const;

/**
 * **The corpus's RDF meaning, as an RML mapping in Turtle.** `manifest` is the text of `fossil.json`.
 *
 * - A vertex type is a `rml:TriplesMap` over its table: the subject is the `identity` column as an
 *   IRI, of the type's class, and each column with a predicate IRI is a predicate-object map whose
 *   object is the column as a literal of {@link DATATYPES}' datatype.
 * - A relation is a `rml:TriplesMap` over a `rml:LogicalView` that inner-joins the edge table's
 *   endpoint keys to the `key` of the two vertex tables they reference, and takes each one's
 *   identity: the source's subject, the relation's predicate, the destination's subject as an IRI.
 * - A type, column or relation without an IRI is not mapped; a type that would map to nothing has no
 *   triples map. A `list<…>` column is not mapped.
 * - Every table is named unqualified, as the corpus names it, through one `rml:Source`, `<#corpus>`:
 *   the consumer says where it attached the corpus by describing that node.
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
  // A relation naming a type with no table has no subject to take at that end, so it is not mapped.
  const relations = manifest.edge_tables.filter(
    (e) => has(e.iri) && vertices.has(e.source.references) && vertices.has(e.destination.references),
  );
  const types = manifest.vertex_tables.filter((t) => has(t.iri) || mapped(t).length > 0);
  const ends = new Set(relations.flatMap((e) => [e.source.references, e.destination.references]));

  const blocks: string[] = [];
  const sources = new Set<string>();
  const source = (table: string): string => {
    if (!sources.has(table)) {
      sources.add(table);
      blocks.push(
        [
          `${node('table', table)} a rml:LogicalSource ;`,
          `  rml:source <#corpus> ;`,
          `  rml:referenceFormulation rml:SQL2008Table ;`,
          `  rml:iterator ${str(ident(table))} .`,
        ].join('\n'),
      );
    }
    return node('table', table);
  };

  for (const table of types) blocks.push(vertexMap(table, source(table.name)));
  for (const name of ends) blocks.push(vertexView(vertices.get(name)!, source(name)));
  for (const edge of relations) {
    const [from, to] = [vertices.get(edge.source.references)!, vertices.get(edge.destination.references)!];
    blocks.push(...edgeMap(edge, source(edge.name), from, to));
  }

  return `${[HEADER, '<#corpus> a rml:Source .', ...blocks].join('\n\n')}\n`;
}

const HEADER = [
  '# The RDF meaning of a fossil/1 corpus, in RML (https://w3id.org/rml/): RML-Core, RML-IO, RML-LV.',
  '# Every table is named as the corpus names it, unqualified. <#corpus> is the catalog a consumer',
  '# attached the corpus under: describe it (a d2rq:Database, say) and resolve the names there.',
  '@prefix rml: <http://w3id.org/rml/> .',
  '@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .',
].join('\n');

/** A vertex type: its identity as the subject, its class, and a literal per mapped column. */
function vertexMap(table: VertexTable, source: string): string {
  const subject = [`rml:reference ${str(table.identity)}`, 'rml:termType rml:IRI'];
  if (has(table.iri)) subject.push(`rml:class ${iri(table.iri)}`);
  const lines = [
    `${node('map', table.name)} a rml:TriplesMap ;`,
    `  rml:logicalSource ${source} ;`,
    `  rml:subjectMap [ ${subject.join(' ; ')} ]`,
  ];
  for (const p of mapped(table)) {
    lines[lines.length - 1] += ' ;';
    lines.push(
      `  rml:predicateObjectMap [ rml:predicate ${iri(p.iri!)} ; rml:objectMap [ ` +
        `rml:reference ${str(p.name)} ; rml:termType rml:Literal ; rml:datatype ${datatype(p.type)} ] ]`,
    );
  }
  return `${lines.join('\n')} .`;
}

/** A vertex table as the parent of a join: its key, and its identity. */
function vertexView(table: VertexTable, source: string): string {
  return [
    `${node('view', table.name)} a rml:LogicalView ;`,
    `  rml:viewOn ${source} ;`,
    `  rml:field ${field(table.key, table.key)} ;`,
    `  rml:field ${field(table.identity, table.identity)} .`,
  ].join('\n');
}

/**
 * A relation: a view of the edge table joined at each end to the vertex table its key references —
 * `src` to the source's `dense_id`, `dst` to the destination's — and a triples map over it.
 */
function edgeMap(edge: EdgeTable, source: string, from: VertexTable, to: VertexTable): string[] {
  const join = (end: (typeof ENDS)[number], key: string, parent: VertexTable): string =>
    [
      `  rml:innerJoin [`,
      `    rml:parentLogicalView ${node('view', parent.name)} ;`,
      `    rml:joinCondition [ rml:child ${str(key)} ; rml:parent ${str(parent.key)} ] ;`,
      `    rml:field ${field(end, parent.identity)}`,
      `  ]`,
    ].join('\n');
  const view = [
    `${node('view', edge.name)} a rml:LogicalView ;`,
    `  rml:viewOn ${source} ;`,
    `  rml:field ${field(edge.source.key, edge.source.key)} ;`,
    `  rml:field ${field(edge.destination.key, edge.destination.key)} ;`,
    `${join('source', edge.source.key, from)} ;`,
    `${join('destination', edge.destination.key, to)} .`,
  ].join('\n');
  const map = [
    `${node('map', edge.name)} a rml:TriplesMap ;`,
    `  rml:logicalSource ${node('view', edge.name)} ;`,
    `  rml:subjectMap [ rml:reference ${str(ENDS[0])} ; rml:termType rml:IRI ] ;`,
    `  rml:predicateObjectMap [ rml:predicate ${iri(edge.iri!)} ; rml:objectMap [ ` +
      `rml:reference ${str(ENDS[1])} ; rml:termType rml:IRI ] ] .`,
  ].join('\n');
  return [view, map];
}

/** An expression field of a view: `name`, the value of `reference`. */
const field = (name: string, reference: string): string =>
  `[ a rml:ExpressionField ; rml:fieldName ${str(name)} ; rml:reference ${str(reference)} ]`;

/** A literal datatype as Turtle: the `xsd:` prefix where the IRI is in it. */
const datatype = (type: string): string => DATATYPES[type]!.replace(XSD, 'xsd:');

/**
 * A node of the mapping itself, relative to wherever the document is kept. The kind is a path segment
 * and the name is percent-encoded, so no table can collide with another kind or with `<#corpus>`.
 */
const node = (kind: 'map' | 'table' | 'view', name: string): string => `<#${kind}/${encodeURIComponent(name)}>`;

/** An IRI as a Turtle `IRIREF`; a character an `IRIREF` cannot hold is written as a `UCHAR`. */
const iri = (value: string): string =>
  `<${value.replace(/[\u0000- <>"{}|^`\\]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`)}>`;

/** A Turtle string literal. */
const str = (value: string): string =>
  `"${value.replace(/[\\"\n\r]/g, (c) => ({ '\\': '\\\\', '"': '\\"', '\n': '\\n', '\r': '\\r' })[c]!)}"`;
