/**
 * `fossil.json` — the one file a reader opens before any Parquet, parsed once, inside `attach`.
 *
 * Its shape is `manifest.gen.ts`, generated from the writer's structs. The one field this side
 * validates is `format`: a reader refuses a format it does not know before it reads a byte of
 * Parquet, and ignores a key it does not know, which is what lets `fossil/1` grow optional fields.
 */

import { FossilError } from '@fossil-lang/types';

import type { EdgeTable, Manifest, PropertyTable, VertexTable } from './manifest.gen.js';

/** Every table the manifest declares — vertices, relations, multi-valued properties — in its order. */
export const tablesIn = (manifest: Manifest): (VertexTable | EdgeTable | PropertyTable)[] => [
  ...manifest.vertex_tables,
  ...manifest.edge_tables,
  ...(manifest.property_tables ?? []),
];

/** The format this reader reads. Anything else is refused at `attach`. */
export const FOSSIL_FORMAT: Manifest['format'] = 'fossil/1';

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
