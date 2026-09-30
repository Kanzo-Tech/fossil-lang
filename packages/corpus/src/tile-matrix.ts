/**
 * The zoom arithmetic — OGC 17-083r4's tile matrices over one vertex type: the payload at `z = Z`
 * and each rung of the cell pyramid below it, coarsest first, every tile listed with its rows and
 * its box.
 */

import { CorpusManifestError, type Channel, type VertexAddress } from './address.js';
import type { CorpusField } from './corpus.js';
import type { Box, ColumnKind } from './expression.js';
import { CorpusReadError } from './sql.js';
import type { AdjacencyEntries, MatrixEntries, TileEntry } from './tile-manifest.js';
import { CELL_COLUMNS } from './vocabulary.generated.js';

/**
 * One vertex type's tile matrices — OGC's `TileMatrixSet`, one per type.
 *
 * `extent` is informative, exactly as the standard's `boundingBox` is: the union of the payload
 * tiles' boxes, and a tile is located by its own box, never by it. `null` for a type with no `x`
 * and `y`, which has tiles and no plane to put them on.
 */
export interface TileMatrixSet {
  readonly type: string;
  readonly extent: Box | null;
  /** The declared coordinate system `x` and `y` are in, by name, or `null` where none is. */
  readonly coordinates: string | null;
  /**
   * The channel a cell's `mode` and `purity` summarise — the cell tree's `mode_channel`, resolved
   * against the type's `channels:` — or `null` where the type has no cells or the tree names none.
   * A view colouring by its `column` colours a cell by `mode`.
   */
  readonly mode: Channel | null;
  /** Coarsest first: `tileMatrices[z].z === z`, and the last is the payload. */
  readonly tileMatrices: readonly TileMatrix[];
}

/**
 * One zoom — OGC's `TileMatrix` on a one-dimensional axis: `tileRows` is `tileWidth` with a height
 * of one, `tiles` is `matrixWidth`, and `shift` plays `cellSize`.
 */
export interface TileMatrix {
  readonly z: number;
  /** `rows` at `z = Z`, where a row is a vertex; `cells` below it, where a row is a cell. */
  readonly kind: 'rows' | 'cells';
  /** How many rows the zoom holds — the type's count, or the rung's cells. */
  readonly count: bigint;
  /** How many `dense_id`s one row stands for, as a power of two: `0` at `Z`, `B + 2(k − 1)` at rung `k`. */
  readonly shift: number;
  /** Rows per tile — the type's `chunk_size` at every zoom. */
  readonly tileRows: number;
  /** Every tile, `tiles[t].tile === t`. None is empty. */
  readonly tiles: readonly TileInfo[];
}

/** One tile: its number, its rows, and its box. */
export interface TileInfo {
  readonly tile: number;
  readonly rows: number;
  /**
   * The tile's `x` and `y` bounds as a {@link Box} — **closed**, `x <= v.x <= x + w`, because they
   * are bounds and not a selection. `null` where the manifest publishes none.
   */
  readonly bbox: Box | null;
}

/** A tile, named — `(type, z, tile)`, OGC's `(tileMatrix, tileRow, tileCol)` on one axis. */
export interface TileAddress {
  readonly type: string;
  readonly z: number;
  readonly tile: number;
}

/** One zoom as the reader reads it: the matrix, and what a read and an edge read need of it. */
export interface Zoom {
  readonly matrix: TileMatrix;
  /** The rung this zoom is, or `null` at `Z`. */
  readonly rung: number | null;
  /** The column a tile's rows are a contiguous range of. */
  readonly key: 'dense_id' | 'cell_id';
  /** Every column a row of this zoom carries, and what its values are. */
  readonly columns: ReadonlyMap<string, ColumnKind>;
  /** The published statistics, by tile. */
  readonly entries: readonly TileEntry[];
  /** At `Z`, the adjacency orientations cut on this type's tiles. */
  readonly adjacencies: readonly AdjacencyEntries[];
  /** Below `Z`, the quotient's entries; `null` where the rung publishes none. */
  readonly quotient: ReadonlyMap<number, TileEntry> | null;
  tileUrl(tile: number): string;
  quotientUrl(tile: number): string | null;
}

/** A type's zooms: the published set, and the reader's own view of each matrix. */
export interface Zooms {
  readonly set: TileMatrixSet;
  readonly zooms: readonly Zoom[];
  /**
   * The zoom an address is in, refusing one outside the set — OGC's 404 as a
   * {@link CorpusReadError}.
   */
  zoom(address: TileAddress): Zoom;
}

/** What a column's values are, from the engine's spelling of its type. */
export function kindOf(type: string): ColumnKind {
  const upper = type.toUpperCase();
  if (upper === 'FLOAT' || upper === 'DOUBLE' || upper === 'REAL') return 'float';
  if (/^U?(TINY|SMALL|BIG|HUGE)?INT(EGER)?$/.test(upper)) return 'integer';
  return 'other';
}

const CELL_KINDS: ReadonlyMap<string, ColumnKind> = new Map(
  CELL_COLUMNS.map((c) => [
    c.name,
    c.dataType === 'float' || c.dataType === 'double'
      ? 'float'
      : /int/.test(c.dataType)
        ? 'integer'
        : 'other',
  ]),
);

const boxOfEntry = (entry: TileEntry): Box | null => {
  const [x0, x1, y0, y1] = [
    entry.lowerBounds['x'],
    entry.upperBounds['x'],
    entry.lowerBounds['y'],
    entry.upperBounds['y'],
  ];
  if (x0 === undefined || x1 === undefined || y0 === undefined || y1 === undefined) return null;
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
};

const union = (boxes: readonly (Box | null)[]): Box | null => {
  if (boxes.length === 0 || boxes.some((b) => b === null)) return null;
  const all = boxes as readonly Box[];
  const x0 = Math.min(...all.map((b) => b.x));
  const y0 = Math.min(...all.map((b) => b.y));
  const x1 = Math.max(...all.map((b) => b.x + b.w));
  const y1 = Math.max(...all.map((b) => b.y + b.h));
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
};

/**
 * **A type's tile matrices, out of its address and its published manifest** — and held against
 * each other: a manifest listing a zoom the document does not declare, or a tile count the
 * arithmetic does not name, is refused while the corpus opens rather than planned from.
 *
 * @throws {CorpusManifestError} when the manifest and the document disagree.
 */
export function zoomsOf(
  address: VertexAddress,
  entries: readonly MatrixEntries[],
  payload: readonly CorpusField[],
): Zooms {
  const rungs = address.cells?.rungs ?? [];
  const top = rungs.length;
  const where = address.tileManifest ?? address.type;
  if (entries.length !== top + 1 || entries.some((m, z) => m.z !== z)) {
    throw new CorpusManifestError(
      `${where} lists zooms ${entries.map((m) => m.z).join(', ')}, and ${address.type} declares ` +
        `${top} rung(s) — so ${top + 1} zoom(s), 0 to ${top}`,
    );
  }
  const tileRows = address.chunkSize;
  const payloadKinds = new Map(payload.map((f) => [f.name, kindOf(f.type)]));
  const zooms = entries.map((m): Zoom => {
    const rung = m.z === top ? null : rungs[top - m.z - 1]!;
    const count = rung === null ? (address.count ?? 0n) : rung.cellCount;
    const tiles = Number((count + BigInt(tileRows) - 1n) / BigInt(tileRows));
    if (m.tiles.length !== tiles || m.tiles.some((t, i) => t.tile !== i)) {
      throw new CorpusManifestError(
        `${where} lists ${m.tiles.length} tile(s) at z = ${m.z}, and ${count} row(s) at ` +
          `${tileRows} a tile are ${tiles}`,
      );
    }
    return {
      matrix: {
        z: m.z,
        kind: rung === null ? 'rows' : 'cells',
        count,
        shift: rung === null ? 0 : rung.shift,
        tileRows,
        tiles: m.tiles.map((t) => ({ tile: t.tile, rows: t.recordCount, bbox: boxOfEntry(t) })),
      },
      rung: rung === null ? null : rung.rung,
      key: rung === null ? 'dense_id' : 'cell_id',
      columns: rung === null ? payloadKinds : CELL_KINDS,
      entries: m.tiles,
      adjacencies: m.adjacencies,
      quotient: rung === null || rung.quotient === null ? null : (m.quotient ?? null),
      tileUrl: (tile) =>
        rung === null ? address.tileUrl(tile) : address.rungTileUrl(rung.rung, tile)!,
      quotientUrl: (tile) => (rung === null ? null : address.quotientTileUrl(rung.rung, tile)),
    };
  });
  const set: TileMatrixSet = {
    type: address.type,
    extent: union(zooms[top]!.matrix.tiles.map((t) => t.bbox)),
    coordinates: address.coordinates,
    mode: address.channels.find((c) => c.name === address.cells?.modeChannel) ?? null,
    tileMatrices: zooms.map((z) => z.matrix),
  };
  return {
    set,
    zooms,
    zoom(at) {
      const zoom = at.type === address.type && Number.isInteger(at.z) ? zooms[at.z] : undefined;
      const tiles = zoom?.matrix.tiles.length ?? 0;
      if (zoom === undefined || !Number.isInteger(at.tile) || at.tile < 0 || at.tile >= tiles) {
        throw new CorpusReadError(
          `(${at.type}, ${at.z}, ${at.tile}) is not a tile of ${address.type}: its matrices are ` +
            `z = 0 to ${top}, holding ${zooms.map((z) => z.matrix.tiles.length).join(', ')} tile(s)`,
        );
      }
      return zoom;
    },
  };
}
