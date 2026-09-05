/**
 * The door, named once — **the only module in this app that spells `corpus.view`.**
 *
 * This file exists for a rename that has not happened yet. `Corpus.view()` becomes
 * `Corpus.frame()` in the consolidation, with a different cost shape underneath it:
 * `ViewCost.tiles`/`ofTiles`/`runs` collapse to `requests` and `bytes` with tiles demoted to a
 * detail, and `cost.read: 'strided' | 'level'` disappears entirely once the pyramid is complete
 * and level 0 is just another projection. Every one of those is a change to the two functions
 * below and to nothing else in `apps/playground/src`.
 *
 * That is not a hypothetical tidiness argument. Before this file there was one call site
 * (`src/tiles.ts`) and the crossfilter was about to add more — a chart that needs the vertex
 * relation, a client that needs the id column — and three call sites into a surface with a
 * scheduled rename is how a one-line change becomes an afternoon. So the coupling is collected
 * here while it is still one line, rather than after it is three.
 *
 * **It deliberately adds no behaviour.** No caching, no defaulting, no cost translation: a
 * wrapper that quietly reinterprets what the door said is worse than the coupling it removes,
 * because then the rename is a one-line change to a function whose semantics nobody trusts.
 * `frame()` forwards its arguments and returns what it was given. The app's own vocabulary —
 * `SliceCost` — is still assembled in `src/tiles.ts`, where the renderer's terms are.
 *
 * ## Where the payload relation comes from
 *
 * {@link vertexRelation} is the third name in this file and it is here for the same reason: the
 * crossfilter's two Mosaic clients query the vertex payload directly by SQL, and composing
 * `read_parquet('<url>')` needs the addressing rather than the drawing path. It reads
 * `corpus.addressing`, which is published for exactly this — «the addressing underneath, for a
 * caller that has outgrown this surface». One place to fix if the tiled tree is ever addressed
 * differently, and the same place the rename lands.
 */
import type { Box, Corpus, View, ViewParams } from '@fossil-lang/corpus';

export type { Box, Corpus, View, ViewParams };

/**
 * Which level of detail a rectangle should be drawn at. **Pure, synchronous, and the caller's.**
 *
 * Separate from {@link frame} because the level is a decision a camera makes *before* it asks for
 * anything, and because «the same rectangle at the same level» has to be expressible or monotone
 * refinement cannot be stated. That is the door's argument, not this file's; this file only keeps
 * the name in one place.
 */
export function levelFor(corpus: Corpus, params: Box & { type?: string; budget: number }): number {
  return corpus.levelFor(params);
}

/**
 * One rectangle, at one level, as the arrays a renderer uploads.
 *
 * **This is the line that becomes `corpus.frame(params)`.** Nothing else in
 * `apps/playground/src` names the method.
 */
export function frame(corpus: Corpus, params: ViewParams): Promise<View> {
  return corpus.view(params);
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
