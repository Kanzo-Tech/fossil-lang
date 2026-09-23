/**
 * The door, named once — **the only module in this app that spells `corpus.frame`.**
 *
 * It was written for a rename that had not happened, and the rename has now happened: `view()`
 * became `frame()`, `ViewCost.tiles`/`ofTiles`/`runs` became `requests` and `bytes` with the tile
 * counts demoted to working, `cost.read` went away as exactly redundant with `matchedAt`, and
 * `corpus.levelFor` went away because the door derives the level from the canvas. Every one of
 * those was a change to this file and to nothing else in `apps/playground/src` — which is what the
 * file was for, and it held. **It did not hold for `scripts/`**, which reach the corpus directly;
 * that is the seam worth knowing about rather than the file having failed.
 *
 * **It deliberately adds no behaviour.** No caching, no defaulting, no cost translation: a
 * wrapper that quietly reinterprets what the door said is worse than the coupling it removes.
 * `frame()` forwards its arguments and returns what it was given. The app's own vocabulary —
 * `SliceCost` — is still assembled in `src/tiles.ts`, where the renderer's terms are.
 *
 * ## Where the payload relation comes from
 *
 * {@link vertexRelation} is the second name in this file and it is here for the same reason: the
 * crossfilter's two Mosaic clients query the vertex payload directly by SQL, and composing
 * `read_parquet('<url>')` needs the addressing rather than the drawing path. It reads
 * `corpus.addressing`, which is published for exactly this — «the addressing underneath, for a
 * caller that has outgrown this surface». One place to fix if the tiled tree is ever addressed
 * differently, and the same place a rename lands.
 */
import type { Box, Corpus, Frame, FrameParams } from '@fossil-lang/corpus';

export type { Box, Corpus, Frame, FrameParams };

/**
 * One rectangle, at one resolution, as the arrays a renderer uploads.
 *
 * Nothing else in `apps/playground/src` names the method.
 */
export function frame(corpus: Corpus, params: FrameParams): Promise<Frame> {
  return corpus.frame(params);
}

/**
 * The vertex payload as something SQL can read — a `read_parquet(…)` ready to be a FROM.
 *
 * The crossfilter's clients are Mosaic clients: they compose their own `SELECT` and hand it to a
 * coordinator, so what they need is a relation expression rather than an answer.
 *
 * `files()` and not a glob, because the two containers disagree about how many there are and the
 * addressing is the thing that knows: one file per tile under `files`, one file in total under
 * `rowgroups` — which is what the bench corpus declares. A list literal covers both without this
 * module branching on a container it has no opinion about. It throws when the manifest declares no
 * `vertex_count`, and that is the right failure: with no count there is no set to enumerate, so
 * there is no relation to hand a chart.
 *
 * Quoted the way every other path in this app reaches SQL: single quotes, doubled inside.
 */
export function vertexRelation(corpus: Corpus, type?: string): string {
  const files = corpus.addressing.vertexType(type).files();
  const list = files.map((url) => `'${url.replace(/'/g, "''")}'`).join(', ');
  return `read_parquet([${list}])`;
}
