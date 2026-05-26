/**
 * Turtle (TTL) serializer — wraps n3.Writer.
 *
 * Moved from `packages/playground/src/turtle/render.ts` to
 * `@fossil-lang/viewer` in Phase 12 plan 12-04 — single source of truth
 * for the view layer. `@fossil-lang/playground` re-exports via a thin
 * shim at `packages/playground/src/turtle/index.ts` so v0.1.x consumers
 * (`import { rowsToTurtle } from '@fossil-lang/playground'`) keep
 * working without code changes.
 *
 * Foundation for PLAY-10 ("Turtle" result tab next to Graph).
 * Per 09-RESEARCH.md "Don't hand-roll": literal escaping + IRI
 * percent-encoding + datatype lexical forms are landmines; n3 handles them.
 *
 * Pure-TS, no DOM, no React — testable from Node Vitest.
 *
 * Writer pinned to n3 ~1.17 (per 09-CONTEXT.md decisions + 09-04 PLAN Task 1):
 * `Writer.end(cb)` fires its callback synchronously when every quad has been
 * added before `.end()` is called (no streaming). We rely on that here — `out`
 * is mutated inside the cb and returned synchronously. See 09-RESEARCH.md
 * Pitfall 6.
 */

import { Writer, DataFactory } from 'n3';

const { namedNode, literal, quad } = DataFactory;

/** A vertex row to be serialized as `<iri> rdf:type <type> ; <p1> <o1> ; ... .` */
export interface VertexRow {
  /** Subject IRI (full IRI, not a CURIE — Writer shortens via prefixes). */
  iri: string;
  /** rdf:type object — full IRI of the class. */
  type: string;
  /**
   * Predicate-IRI → literal object map. `null` values are skipped.
   * - `string` → plain literal (xsd:string).
   * - `number` → xsd:integer (if integral) or xsd:double otherwise.
   * - `boolean` → xsd:boolean.
   */
  props: Record<string, string | number | boolean | null>;
}

/** An edge row: subject IRI → predicate IRI → object IRI. */
export interface EdgeRow {
  /** Subject IRI. */
  src: string;
  /** Predicate IRI. */
  pred: string;
  /** Object IRI. */
  dst: string;
}

const RDF_TYPE_IRI = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const XSD_NS = 'http://www.w3.org/2001/XMLSchema#';

/**
 * Serialize vertex + edge rows into Turtle text.
 *
 * @param vertices  Vertex rows (each emits 1 rdf:type quad + N property quads).
 * @param edges     Edge rows (each emits 1 quad).
 * @param prefixes  Prefix declarations to embed in the `@prefix` block — keys
 *                  are CURIE prefixes (`ex`, `rdf`, `xsd`, ...), values full IRIs.
 *                  Callers MUST supply the prefixes they want shortened; the
 *                  writer does not auto-discover them from the data.
 * @returns         Turtle text. Synchronous (Writer.end fires cb sync because
 *                  all quads are added before .end() is called).
 */
export function rowsToTurtle(
  vertices: VertexRow[],
  edges: EdgeRow[],
  prefixes: Record<string, string>,
): string {
  const writer = new Writer({ prefixes });

  // Find the xsd namespace — caller's xsd: prefix wins; fall back to W3C default.
  const xsdNs = prefixes['xsd'] ?? XSD_NS;

  for (const v of vertices) {
    // rdf:type
    writer.addQuad(
      quad(namedNode(v.iri), namedNode(RDF_TYPE_IRI), namedNode(v.type)),
    );

    // properties
    for (const [predIri, objVal] of Object.entries(v.props)) {
      if (objVal === null) continue;

      let objTerm;
      if (typeof objVal === 'string') {
        objTerm = literal(objVal);
      } else if (typeof objVal === 'number') {
        const dt = Number.isInteger(objVal) ? 'integer' : 'double';
        objTerm = literal(String(objVal), namedNode(xsdNs + dt));
      } else {
        // boolean
        objTerm = literal(String(objVal), namedNode(xsdNs + 'boolean'));
      }

      writer.addQuad(quad(namedNode(v.iri), namedNode(predIri), objTerm));
    }
  }

  for (const e of edges) {
    writer.addQuad(
      quad(namedNode(e.src), namedNode(e.pred), namedNode(e.dst)),
    );
  }

  let out = '';
  let cbErr: Error | null = null;
  writer.end((err, result) => {
    if (err) {
      cbErr = err;
      return;
    }
    out = result;
  });

  if (cbErr) throw cbErr;
  return out;
}
