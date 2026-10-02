/**
 * `fossil.json` — the one file a reader opens before any Parquet, parsed once, inside `open`.
 *
 * Nothing here is exported past the package: a caller reads the manifest as the two relations `open`
 * makes of it, so the shape is restated once — by the writer, `fossil_sinks::manifest`, and its
 * `fossil.schema.json` — and this side declares only the fields `open` reads. The one field it
 * validates is `format`: a reader refuses a format it does not know before it reads a byte of
 * Parquet, and ignores a key it does not know, which is what lets `fossil/1` grow optional fields.
 */

import { FossilError } from '@fossil-lang/types';

/** The format this reader reads. Anything else is refused at `open`. */
export const FOSSIL_FORMAT = 'fossil/1';

/** One column of a table, as the manifest declares it. */
export interface Property {
  readonly name: string;
  /** The manifest's type word: `uint32`, `string`, `int32`, … — the writer's vocabulary. */
  readonly type: string;
  readonly iri?: string;
  readonly nullable?: boolean;
  /** What a column the writer emits IS — `corpus.bnf`'s role. Absent on a program's column. */
  readonly role?: 'address' | 'identity' | 'endpoint';
}

/** A vertex table: one per vertex type, a contiguous range of `dense_id` in manifest order. */
export interface VertexTable {
  readonly name: string;
  readonly iri?: string;
  /** Relative to the corpus root. */
  readonly path: string;
  readonly record_count: number;
  readonly properties: readonly Property[];
}

/** An edge table — one per relation `(source type, label, destination type)`. */
export interface EdgeTable {
  readonly name: string;
  readonly iri?: string;
  readonly path: string;
  readonly source: { readonly references: string };
  readonly destination: { readonly references: string };
  readonly record_count: number;
  readonly properties: readonly Property[];
}

/** `fossil.json`, as far as `open` reads it. */
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
