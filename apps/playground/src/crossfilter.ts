/**
 * The crossfilter: two clients over one coordinator, and the mask the canvas wears.
 *
 * A coordinator with one client is a crossfilter with nothing to cross, which is exactly the
 * argument that kept Mosaic out of this app. So there are two, and they are different *kinds* of
 * client on purpose:
 *
 * - **the histogram** (`src/Histogram.tsx`) — a chart whose `x` IS a column, so its brush publishes
 *   an interval, `birth_year BETWEEN a AND b`, and the database can evaluate it directly.
 * - **the canvas** (`IdSetClient`, wired below) — a view whose positions are *not* in the database
 *   in any form a predicate can reach. Its `x` and `y` are columns, but what is on screen is a
 *   rectangle-and-level answer from `corpus.view`, decimated by `dense_id % 2^k` and cut off at a
 *   mark budget. No `WHERE` over columns describes «what this camera drew». So it goes the other
 *   way: it asks which ids survive the page's filters and fades everything else. Ids out, a mask
 *   over resident tiles. That is precisely the shape `IdSetClient` exists for, which is why it is
 *   used rather than re-derived here.
 *
 * ## Why the mask is a `Uint8Array` indexed by `dense_id` and not a `Set`
 *
 * Because `dense_id` is not an arbitrary key — it is a **rank**. The corpus format's whole
 * addressing argument is that `dense_id` is the vertex's position in Morton order, dense and
 * contiguous from zero, which is what makes a rectangle a range of tiles. So the id space is
 * already a perfect array index: a `Uint8Array(count)` is one byte per vertex — 1 MB at the bench
 * corpus's million — filled by a linear pass with no hashing, and read in `O(1)` with no boxing.
 * A `Set<number>` of a million survivors costs an order of magnitude more to build and is slower
 * to probe, to answer the same question.
 *
 * ## The ceiling, stated rather than hidden
 *
 * `IdSetClient` says it itself: «its cost is the length of the `IN` list, which is the real ceiling
 * on the whole approach». Here the list is the *query* side, not the publish side — every filter
 * change hands back one id per surviving vertex, so an unbrushed histogram over the million-vertex
 * corpus returns a million ids. That is a full column scan in DuckDB plus a million-element array
 * in JS, and it is the dominant cost of the whole feature. It is measured on every update and
 * printed beside the chart rather than described, because a crossfilter that feels slow and does
 * not say why is worse than one that does.
 *
 * **What would remove it** is a filter the canvas could evaluate without asking: if `corpus.view`
 * carried the filtered column back beside `categories`, the mask would be a comparison per drawn
 * vertex — twenty thousand, not a million — and no id would cross the boundary at all. That is a
 * change to the door and it is not made here.
 */
import { denseOf, type Resident } from '@kanzo-tech/graph';
import { IdSetClient, Selection, type Coordinator } from '@kanzo-tech/mosaic';

import { vertexRelation, type Corpus } from './frame.js';

/** The view both clients name. A view and not an inline relation, for one measured reason:
 * `Query.from('read_parquet([…])')` quotes its argument — `FROM "read_parquet([…])"` — because a
 * bare string is a table *reference* in mosaic-sql's grammar, and `IdSetClient.table` is a string.
 * Naming a real view is the fix that needs no `sql` template threaded through somebody else's
 * option type, and it costs one statement at open. */
export const XF_VIEW = 'xf_vertex';

/** What one filter pass cost, for the panel to print. */
export interface CrossfilterCost {
  /** Vertices that survived the page's filters. */
  survivors: number;
  /** Vertices in the relation — the denominator, and the size of the mask. */
  population: number;
  /** Wall clock from the coordinator's answer to the mask being ready. */
  maskMs: number;
}

export interface CrossfilterOptions {
  coordinator: Coordinator;
  corpus: Corpus;
  /** Which vertex type. The crossfilter is over one type, like everything else the canvas draws. */
  type?: string;
  /** How many vertices the type has — the mask's length, off the manifest. */
  count: number;
  /** A new mask is ready. `null` means «no filter is active», which is not the same as «none
   * survived»: an all-ones mask and no mask draw identically, but only one of them is a statement
   * about the data. */
  onMask(mask: Uint8Array | null, cost: CrossfilterCost): void;
}

export interface Crossfilter {
  /** What the histogram brushes into, and what the canvas client is filtered by. */
  readonly filter: Selection;
  /** Where the canvas would publish, if it grew a lasso. Held so the wiring is symmetrical and so
   * a future selection gesture has a destination that already exists. */
  readonly selection: Selection;
  /** The relation both clients read. */
  readonly table: string;
  /** Disconnect the canvas client. The histogram owns its own lifetime. */
  destroy(): void;
}

/**
 * Create the view, connect the canvas client, and hand back the two selections.
 *
 * `Selection.crossfilter()` and not `Selection.intersect()`: a crossfilter exempts each client from
 * its own clause, which is what lets a brush be *widened* after it has collapsed the chart under
 * it. With `intersect`, dragging a histogram brush down to one bin leaves that chart with one bar
 * and nothing to drag back out of.
 *
 * The canvas client declines that exemption on its own side — see `IdSetClient.publish`, which
 * passes an empty `clients` set — because a view that *fades* an excluded vertex rather than
 * removing it does not need it: the vertex is still on screen and still selectable, and the fade is
 * the brush.
 */
export async function openCrossfilter(options: CrossfilterOptions): Promise<Crossfilter> {
  const { coordinator, corpus, count, onMask, type } = options;

  const relation = vertexRelation(corpus, type);
  // Only the columns the crossfilter reads. `subject` is the widest column in the corpus and
  // nothing here filters or draws by it, so projecting it into the view would put it in the way of
  // every scan the two clients make.
  await coordinator.exec(
    `CREATE OR REPLACE VIEW ${XF_VIEW} AS ` +
      `SELECT dense_id, birth_year, postcode, cluster_id FROM ${relation}`,
  );

  const filter = Selection.crossfilter();
  const selection = Selection.crossfilter();

  const canvas = new IdSetClient({
    table: XF_VIEW,
    idField: 'dense_id',
    filterBy: filter,
    as: selection,
    onSurvivors: (ids) => {
      const started = performance.now();
      // Sized off the manifest's `vertex_count`, so an id is its own index. A survivor outside that
      // range would be a corpus whose `dense_id` is not a rank, which is the property the format
      // guarantees — but the bounds check is one comparison against a million and buys a wrong
      // picture instead of a thrown exception if it ever stops being true.
      const mask = new Uint8Array(count);
      let held = 0;
      for (const id of ids) {
        const index = Number(id);
        if (index >= 0 && index < count) {
          mask[index] = 1;
          held += 1;
        }
      }
      // Everything survived, so nothing is filtered. Reported as `null` rather than as a full mask
      // because the canvas can then skip the re-colour entirely — the common case, on open and
      // after a brush is cleared, is the one that should cost nothing.
      const filtered = held < count;
      onMask(filtered ? mask : null, {
        survivors: held,
        population: count,
        maskMs: performance.now() - started,
      });
    },
  });

  coordinator.connect(canvas);

  return {
    filter,
    selection,
    table: XF_VIEW,
    destroy: () => coordinator.disconnect(canvas),
  };
}

/**
 * The mask, applied to what is drawn — the only place this app touches point colours.
 *
 * cosmos.gl has no greyout of its own in 3.4.1 (`selectPointsByIndices` is not in its surface;
 * `getPointColors`/`setPointColors` are), so the fade is a re-upload of the colours the renderer
 * already holds with alpha scaled on the excluded rows. Scaling rather than replacing is what keeps
 * this compatible with `@kanzo-tech/graph`'s colouring: the categorical palette, the fold into
 * `--chart-capacity` slots and the muted token are all still kanzo's decisions, and this multiplies
 * one channel of the result.
 *
 * `baseline` is the undimmed upload, captured by the caller when the slice changed. It has to be
 * kept: reading `getPointColors()` after a fade and fading that again compounds, and four brush
 * moves would take a live vertex to invisible.
 *
 * Returns whether anything was uploaded — `false` when the buffer does not match the resident set,
 * which happens for a frame or two after a camera move while the renderer catches up. The caller
 * retries rather than drawing a mask against the wrong vertices.
 */
export function paintMask(
  graph: {
    getPointColors(): Float32Array;
    setPointColors(colors: Float32Array): void;
    render(alpha?: number, transition?: number): void;
  },
  resident: Resident,
  baseline: Float32Array,
  mask: Uint8Array | null,
  dim: number,
): boolean {
  const colors = graph.getPointColors();
  // The renderer's buffer and the resident map disagree for a frame after an answer arrives. A
  // mask painted then would fade whichever vertices happen to sit at those indices now, which is
  // the buffer-index-is-not-an-identity failure `resident.ts` is entirely about.
  if (colors.length !== baseline.length || baseline.length < resident.size * 4) return false;

  const next = new Float32Array(baseline);
  if (mask !== null) {
    for (let i = 0; i < resident.size; i += 1) {
      const vertex = resident.at(i);
      if (vertex === undefined) continue;
      const id = denseOf(vertex);
      if (id >= 0 && id < mask.length && mask[id] === 1) continue;
      // Alpha only. Touching rgb would move an excluded vertex to a colour that means something
      // else in the legend beside it.
      next[i * 4 + 3] = (baseline[i * 4 + 3] ?? 1) * dim;
    }
  }
  graph.setPointColors(next);
  // `render(undefined, 0)` snaps: keep the current simulation alpha — which is zero here, the
  // simulation being off — and skip the transition, so a brush drag repaints per frame instead of
  // animating each step of itself.
  graph.render(undefined, 0);
  return true;
}
