/**
 * What the reader knows about each tile before it reads one — Iceberg's manifest: the per-tile
 * statistics the writer publishes, read once per type while the corpus opens ({@link published}).
 * The footers they are read off are the writer's; a reader never sweeps them.
 */

import { CorpusManifestError, type Direction } from './address.js';
import type { Metrics } from './expression.js';

/** One tile's published entry: its number and Iceberg's four statistics. */
export interface TileEntry extends Metrics {
  readonly tile: number;
}

/** One adjacency orientation cut on a type's tiles, as the manifest lists it — sparse. */
export interface AdjacencyEntries {
  readonly edgeType: string;
  readonly srcType: string;
  readonly dstType: string;
  readonly alignedBy: Direction;
  readonly tiles: ReadonlyMap<number, TileEntry>;
}

/** One zoom's entries: its tiles, dense, and the edge sets its tiles address. */
export interface MatrixEntries {
  readonly z: number;
  readonly tiles: readonly TileEntry[];
  readonly adjacencies: readonly AdjacencyEntries[];
  /** The rung's quotient, sparse, or `null` where the manifest lists none. */
  readonly quotient: ReadonlyMap<number, TileEntry> | null;
}

/**
 * **A type's tile manifest, parsed** — `crates/fossil-sinks/src/tiles.rs, TileManifest` read back.
 *
 * A bound that is an integer beyond 2^53 is dropped rather than kept rounded: `JSON.parse` rounds
 * it, and a rounded bound can prune the tile holding the match, where an absent one only costs the
 * pruning.
 */
export function published(text: string, where: string): readonly MatrixEntries[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    throw new CorpusManifestError(`${where} is not JSON: ${(cause as Error).message}`);
  }
  const record = (value: unknown): Record<string, unknown> =>
    typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
  const numbers = (value: unknown, exact: boolean): Record<string, number> => {
    const out: Record<string, number> = {};
    for (const [key, v] of Object.entries(record(value))) {
      if (typeof v !== 'number' || (exact && Number.isInteger(v) && !Number.isSafeInteger(v))) continue;
      out[key] = v;
    }
    return out;
  };
  const entry = (value: unknown): TileEntry => {
    const e = record(value);
    return {
      tile: Number(e['tile']),
      recordCount: Number(e['record_count']),
      nullValueCounts: numbers(e['null_value_counts'], false),
      lowerBounds: numbers(e['lower_bounds'], true),
      upperBounds: numbers(e['upper_bounds'], true),
    };
  };
  const sparse = (list: unknown): Map<number, TileEntry> =>
    new Map((Array.isArray(list) ? list : []).map((t) => [Number(record(t)['tile']), entry(t)]));
  const matrices = record(parsed)['matrices'];
  if (!Array.isArray(matrices)) throw new CorpusManifestError(`${where} lists no matrices`);
  return matrices.map((m) => {
    const matrix = record(m);
    const quotient = matrix['quotient'];
    return {
      z: Number(matrix['z']),
      tiles: (Array.isArray(matrix['tiles']) ? matrix['tiles'] : []).map(entry),
      adjacencies: (Array.isArray(matrix['adjacencies']) ? matrix['adjacencies'] : []).map((a) => {
        const adjacency = record(a);
        return {
          edgeType: String(adjacency['edge_type']),
          srcType: String(adjacency['src_type']),
          dstType: String(adjacency['dst_type']),
          alignedBy: adjacency['aligned_by'] === 'dst' ? 'dst' : 'src',
          tiles: sparse(adjacency['tiles']),
        };
      }),
      quotient: Array.isArray(quotient) ? sparse(quotient) : null,
    };
  });
}
