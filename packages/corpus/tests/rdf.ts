import type { Manifest, Property } from '../src/manifest.gen.js';

/**
 * **What a corpus holds as RDF, and what `triples` says it holds, as two sets of N-Triples** — the
 * test's oracle and its reader. {@link held} is SQL a test states over the views `open` made and the
 * manifest, not the view: so `triples` is held to something it did not compute.
 */

const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const RR_IRI = 'http://www.w3.org/ns/r2rml#IRI';

/** One triple, spelled as N-Triples spells it, so two sets compare as strings. */
export type Triple = string;

export const nt = {
  iri: (value: string): string => `<${value}>`,
  literal: (value: string, datatype: string): string => `${JSON.stringify(value)}^^<${datatype}>`,
};

type Rows = (sql: string) => Promise<Record<string, unknown>[]>;

/**
 * The rows of a `triples` relation, as N-Triples. A row that is not a triple this format can make — a
 * subject that is not an IRI, an object kind other than `I` and `L`, a datatype on an IRI or a
 * language on anything — is refused by name rather than spelled.
 */
export async function read(relation: string, query: Rows): Promise<Set<Triple>> {
  const out = new Set<Triple>();
  for (const r of await query(`SELECT * FROM ${relation}`)) {
    const [sk, s, p, ok, o, od, ol] = ['s_k', 's_v', 'p', 'o_k', 'o_v', 'o_d', 'o_l'].map((c) => String(r[c]));
    if (sk !== 'I' || ol !== '' || !((ok === 'I' && od === '') || (ok === 'L' && od !== ''))) {
      throw new Error(`not a triple of a corpus: ${JSON.stringify(r)}`);
    }
    out.add(`${nt.iri(s!)} ${nt.iri(p!)} ${ok === 'I' ? nt.iri(o!) : nt.literal(o!, od!)}`);
  }
  return out;
}

/**
 * **What the corpus holds as RDF**, written against the manifest and the views `open` made. A
 * column's term is the one the manifest declares; where it declares none, the caller states the
 * datatype of each type word its corpus uses, and a word it did not state is refused.
 */
export async function held(
  manifest: Manifest,
  catalog: string,
  query: Rows,
  typed: Readonly<Record<string, string>>,
): Promise<Set<Triple>> {
  const out = new Set<Triple>();
  /** Each row of `sql` — a `subject` and a `v` — as the triple `p` declares. */
  const values = async (p: Property, sql: string) => {
    const datatype = p.datatype ?? typed[p.type];
    if (p.term_type !== RR_IRI && datatype === undefined) throw new Error(`a ${p.type} column the test did not say the datatype of`);
    const object = (v: string) => (p.term_type === RR_IRI ? nt.iri(v) : nt.literal(v, datatype!));
    for (const r of await query(sql)) out.add(`${nt.iri(String(r.subject))} ${nt.iri(p.iri!)} ${object(String(r.v))}`);
  };
  for (const t of manifest.vertex_tables) {
    for (const r of await query(`SELECT subject FROM ${catalog}."${t.name}"`)) {
      if (t.iri) out.add(`${nt.iri(String(r.subject))} <${RDF_TYPE}> ${nt.iri(t.iri)}`);
    }
    for (const p of t.properties.filter((p) => p.iri !== undefined)) {
      await values(p, `SELECT subject, "${p.name}"::VARCHAR AS v FROM ${catalog}."${t.name}" WHERE "${p.name}" IS NOT NULL`);
    }
  }
  for (const t of manifest.property_tables ?? []) {
    for (const p of t.properties.filter((p) => p.iri !== undefined)) {
      await values(
        p,
        `SELECT s.subject, e."${p.name}"::VARCHAR AS v FROM ${catalog}."${t.name}" e
           JOIN ${catalog}."${t.source.references}" s ON e.src = s.dense_id`,
      );
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
