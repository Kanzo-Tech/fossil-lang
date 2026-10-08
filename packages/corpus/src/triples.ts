/**
 * `"<name>".triples`: the corpus as RDF, one relation a SHACL engine or any reader of triples reads
 * with no fossil code. A pure function of `fossil.json`, as the other two catalog views are.
 *
 * The meaning was always in the manifest: a vertex table has a class IRI and an identity column, a
 * column may carry a predicate IRI and the term its shape declared, an edge table has a predicate IRI
 * and the two vertex tables its endpoints reference. This states it once, as SQL over the views
 * `attach` made. `/docs/format/reading/rdf` is the specification of what comes out.
 */

import type { Manifest, Property, PropertyTable, VertexTable } from './manifest.gen.js';
import { ident, lit } from './sql.js';

const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const RR_IRI = 'http://www.w3.org/ns/r2rml#IRI';
const XSD = 'http://www.w3.org/2001/XMLSchema#';

/**
 * The columns of the relation, in order, named as SPARQL 1.1 Query Results JSON names a term's
 * members (§3.2.2): the subject's type (`I`) and value (its IRI), the predicate IRI, then the
 * object's type (`I` or `L`), value (IRI or lexical form), datatype IRI (`''` for an IRI) and
 * language tag (`''`; a corpus states none).
 */
export const TRIPLES_COLUMNS = ['s_type', 's_value', 'p', 'o_type', 'o_value', 'o_datatype', 'o_lang'] as const;

const has = (iri: string | undefined): iri is string => iri !== undefined && iri !== '';

/** The predicate-mapped columns of a table: the program's, with an IRI. */
const mapped = (table: VertexTable | PropertyTable): Property[] =>
  table.properties.filter((p) => p.role === undefined && has(p.iri));

/**
 * The natural RDF datatype of a manifest type word (R2RML §10.2): XSD's for the SQL types it names,
 * and `xsd:string` otherwise.
 */
function natural(type: string): string {
  if (/^u?int(8|16|32|64)$/.test(type)) return `${XSD}integer`;
  if (type === 'float' || type === 'double') return `${XSD}double`;
  if (type.startsWith('decimal')) return `${XSD}decimal`;
  const word = { bool: 'boolean', date: 'date', time: 'time', timestamp: 'dateTime', binary: 'hexBinary' }[type];
  return `${XSD}${word ?? 'string'}`;
}

/**
 * A value's natural lexical form, as SQL (R2RML §10.2): its text, with the corrections a DuckDB cast
 * needs — a timestamp's `T`, a binary's hex, and XSD's spelling of the infinities and NaN.
 */
function lexical(value: string, type: string): string {
  if (type === 'timestamp') return `replace(${value}::VARCHAR, ' ', 'T')`;
  if (type === 'binary') return `hex(${value})`;
  if (type === 'float' || type === 'double') {
    return `CASE WHEN isnan(${value}) THEN 'NaN' WHEN isinf(${value}) THEN (CASE WHEN ${value} > 0 THEN 'INF' ELSE '-INF' END) ELSE ${value}::VARCHAR END`;
  }
  return `${value}::VARCHAR`;
}

/** The object columns of a value, as the term its column declares. */
function object(value: string, p: Property): string {
  if (p.term_type === RR_IRI) return `'I', ${value}::VARCHAR, '', ''`;
  return `'L', ${lexical(value, p.type)}, ${lit(p.datatype ?? natural(p.type))}, ''`;
}

/**
 * The relation, as one `SELECT`: a branch per vertex class, per mapped column, per relation and per
 * multi-valued property, each over the views `relation` names. A null value makes no triple; what has
 * no IRI is not mapped.
 */
export function triplesOf(manifest: Manifest, relation: (table: string) => string): string {
  const vertices = new Map(manifest.vertex_tables.map((t) => [t.name, t]));
  const header = TRIPLES_COLUMNS.map((c) => `CAST(NULL AS VARCHAR) AS ${ident(c)}`).join(', ');
  const branches = [`SELECT ${header} WHERE false`];

  for (const t of manifest.vertex_tables) {
    const subject = `'I', ${ident(t.identity)}`;
    if (has(t.iri)) branches.push(`SELECT ${subject}, ${lit(RDF_TYPE)}, 'I', ${lit(t.iri)}, '', '' FROM ${relation(t.name)}`);
    for (const p of mapped(t)) {
      const value = ident(p.name);
      branches.push(`SELECT ${subject}, ${lit(p.iri!)}, ${object(value, p)} FROM ${relation(t.name)} WHERE ${value} IS NOT NULL`);
    }
  }
  // A relation or property naming a type with no table has no subject at that end, so it is not mapped.
  for (const e of manifest.edge_tables) {
    const [from, to] = [vertices.get(e.source.references), vertices.get(e.destination.references)];
    if (!has(e.iri) || from === undefined || to === undefined) continue;
    branches.push(
      `SELECT 'I', s.${ident(from.identity)}, ${lit(e.iri)}, 'I', d.${ident(to.identity)}, '', '' ` +
        `FROM ${relation(e.name)} AS e ` +
        `JOIN ${relation(from.name)} AS s ON e.${ident(e.source.key)} = s.${ident(from.key)} ` +
        `JOIN ${relation(to.name)} AS d ON e.${ident(e.destination.key)} = d.${ident(to.key)}`,
    );
  }
  for (const t of manifest.property_tables ?? []) {
    const [p] = mapped(t);
    const from = vertices.get(t.source.references);
    if (p === undefined || from === undefined) continue;
    const value = `e.${ident(p.name)}`;
    branches.push(
      `SELECT 'I', s.${ident(from.identity)}, ${lit(p.iri!)}, ${object(value, p)} ` +
        `FROM ${relation(t.name)} AS e ` +
        `JOIN ${relation(from.name)} AS s ON e.${ident(t.source.key)} = s.${ident(from.key)} ` +
        `WHERE ${value} IS NOT NULL`,
    );
  }
  return branches.join('\nUNION ALL ');
}

