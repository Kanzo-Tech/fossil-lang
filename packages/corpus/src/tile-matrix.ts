/**
 * The zoom arithmetic — OGC 17-083r4's tile matrices over one vertex type, which today is the level
 * a canvas can show and will be the matrices `tileMatrix` lists.
 */

import { strideBits, type VertexAddress } from './address.js';
import { CorpusReadError } from './sql.js';

/**
 * The canvas a frame is drawn into. **A number a caller already has** — this was a budget in
 * marks, which is a number nothing outside this package produces and every host invented
 * differently out of the same two facts: how big the canvas is and how big a mark is.
 */
export interface Pixels {
  readonly w: number;
  readonly h: number;
}

/**
 * **Which level a canvas of this size can show** — the derivation that used to be a second call.
 *
 * `w · h` marks, because a pixel is the finest thing a canvas tells apart. What the rectangle
 * holds is estimated as the TILES it touches times `chunk_size`, capped by the type's own count
 * (`/docs/design/camera` measures why tiles and not area); a type with no geometry falls back to
 * its whole tile count, which the ceiling turns into the level the whole type needs. The
 * logarithm is in the pyramid's base through {@link strideBits}, because a level drops
 * `strideOf(k)` and not `2^k`. Synchronous over footers `open` already bought: `touched` is the tiles
 * the rectangle selects from them, and `null` for a type they do not bound.
 *
 * **This is why the level stopped being the caller's**, reversing `/docs/design/one-door`: that
 * argument rested on a partial pyramid, where a derived level could name an artefact nobody
 * wrote. It is complete now, so every derived level is answerable — and what the reversal owed
 * the argument is {@link FrameParams.level}, which keeps refinement sayable.
 */
export const levelForCanvas = (
  type: VertexAddress,
  touched: number | null,
  pixels: Pixels | undefined,
): number => {
  if (pixels === undefined) {
    throw new CorpusReadError(
      'a frame needs a resolution: pass `pixels` for the canvas it is drawn into, or `level` to name one',
    );
  }
  const marks = pixels.w * pixels.h;
  if (!(marks > 0)) {
    throw new CorpusReadError(
      `a canvas is positive in both directions; got ${String(pixels.w)}×${String(pixels.h)}`,
    );
  }
  const tiles = touched ?? Number(type.tiles ?? 0n);
  const ceiling = type.count === null ? Number.POSITIVE_INFINITY : Number(type.count);
  const estimate = Math.min(tiles * type.chunkSize || 0, ceiling);
  if (!(estimate > marks)) return 0;
  return Math.ceil(Math.log2(estimate / marks) / strideBits(1));
};
