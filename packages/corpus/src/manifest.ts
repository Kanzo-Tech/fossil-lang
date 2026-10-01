/**
 * `fossil.json` — the one file a reader opens before any Parquet, parsed once.
 *
 * The shape mirrors the writer's (`fossil_sinks::manifest`, which serialises it), and the only
 * field this side validates is `format`: a reader refuses a format it does not know before it reads
 * a byte of Parquet, and ignores a key it does not know, which is what lets `fossil/1` grow
 * optional fields without a reader changing.
 */

import { FossilError } from '@fossil-lang/types';

/** The format this reader reads. Anything else is refused at `open`. */
export const FOSSIL_FORMAT = 'fossil/1';


/** One column of a table, as the manifest declares it. */
export interface Property {
  readonly name: string;
  /** The manifest's type word: `uint32`, `string`, `float`, `int32`, … — the writer's vocabulary. */
  readonly type: string;
  readonly iri?: string;
  readonly nullable?: boolean;
}

/**
 * Which two columns a vertex table is drawn at: the layout's `x`/`y`, or two of the program's own
 * (`lon`/`lat`). A table without one is not drawn.
 */
export interface Position {
  readonly by: 'layout' | 'program';
  readonly x: string;
  readonly y: string;
}

/** A vertex table — one per vertex type, and a view of that name once the corpus is open. */
export interface VertexTable {
  readonly name: string;
  readonly iri?: string;
  /** Relative to the corpus root. */
  readonly path: string;
  /** `dense_id`: global over the graph, gapless, and the drawn vertices first. */
  readonly key: string;
  /** `subject`: what survives a rewrite, where `key` does not. */
  readonly identity: string;
  readonly record_count: number;
  readonly properties: readonly Property[];
  readonly position?: Position;
}

/** One end of an edge table: the column holding it, and the vertex table whose key it is. */
export interface Endpoint {
  readonly key: string;
  readonly references: string;
}

/** An edge table — one per relation `(source type, label, destination type)`. */
export interface EdgeTable {
  /** Unique; the `label` repeats across relations. */
  readonly name: string;
  readonly label: string;
  readonly iri?: string;
  readonly path: string;
  readonly source: Endpoint;
  readonly destination: Endpoint;
  readonly record_count: number;
  readonly properties: readonly Property[];
}

/** `fossil.json`. */
export interface Manifest {
  readonly format: typeof FOSSIL_FORMAT;
  readonly vertex_tables: readonly VertexTable[];
  readonly edge_tables: readonly EdgeTable[];
}

/**
 * Parse `fossil.json`. `where` names it in an error.
 *
 * @throws {FossilError} `corpus/not-json` for text that is not JSON, `corpus/unsupported-format` for a
 *   `format` other than `fossil/1`.
 */
export function parseManifest(text: string, where: string): Manifest {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch (cause) {
    throw FossilError.of('corpus/not-json', { path: where }, { cause });
  }
  const format = (json as { format?: unknown } | null)?.format;
  if (format !== FOSSIL_FORMAT) {
    throw FossilError.of(
      'corpus/unsupported-format',
      { path: where, format: typeof format === 'string' ? format : String(JSON.stringify(format)) },
    );
  }
  return json as Manifest;
}
