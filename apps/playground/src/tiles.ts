/**
 * A `BoundedSource` over a fossil corpus — the seam, and nothing but the seam.
 *
 * **kanzo owns the contract and the renderer; fossil owns the corpus.** `@kanzo-tech/graph` says a
 * camera is *a tile brought by a computed URL, which is another source*; this is that source, and
 * what is left in it is a translation between two vocabularies: a `Viewport` that may be `±Infinity`
 * becomes a finite `Box`, a `Frame` becomes a `Slice`, a cost becomes the ledger beside the canvas.
 *
 * **Everything else went through the door.** Tile selection, the `dense_id` ranges, the box
 * predicate, the stride, the `held`/`pool`/`vis`/`span`/`anchor` CTEs and the row→typed-array
 * assembly were all written out here once; they are `corpus.frame` now, which is the arithmetic
 * `@fossil-lang/corpus` publishes so that no reader re-derives it — see [`/docs/design/one-door`].
 * ONE call and not two: the door takes the canvas and derives the level, because a camera has
 * pixels and had to invent a budget in marks to talk to the old surface. *The same rectangle at
 * the same level* is still sayable — `level` names one outright — and that is what keeps monotone
 * refinement statable, which is the argument the second call used to carry.
 * No registration step either: the door composes `read_parquet('<url>')` against real addresses,
 * and so does everything else this app reads with.
 *
 * ## Why no Mosaic
 *
 * `duckBoundedSource` is a client of a Mosaic `Coordinator`, which is how a graph ends up inside the
 * same crossfilter as the charts beside it. There are no charts beside this one. `BoundedSource` is
 * one method, the app already owns a DuckDB connection, and adding `@uwdata/mosaic-core`,
 * `@uwdata/mosaic-sql` and `@kanzo-tech/mosaic` to get a coordinator with one client in it would buy
 * a crossfilter with nothing to cross — and a SECOND DuckDB-WASM to boot it against. The contract
 * invites this explicitly: «the wiring between a particular source and this contract belongs at the
 * call site.»
 */
import type { Box, Corpus, Frame } from '@fossil-lang/corpus';
import {
  BOUNDED_DEFAULTS,
  denseOf,
  typeOf,
  vertexId,
  type BoundedSource,
  type Slice,
  type SliceRequest,
  type Viewport,
  type VertexId,
} from '@kanzo-tech/graph';

import { frame } from './frame.js';
import { extentOf, type Rect, type TileBox } from './stream.js';

/** What one answer cost, in the terms the panel beside the canvas is already reporting. */
export interface SliceCost {
  /** Tiles the rectangle and the pins selected, and the tiles the type has. */
  tiles: number;
  ofTiles: number;
  /**
   * Range requests — maximal runs of adjacent tiles, one request each — and the bytes they hold.
   *
   * **Derived, and it says so.** DuckDB issues the real requests from inside its Worker, where this
   * thread cannot weigh them; the ledger above the canvas weighs its own, off the footer.
   */
  requests: number;
  bytes: number;
  /**
   * **The rectangle this source was asked about**, in the corpus's own coordinates.
   *
   * On the ledger because the one thing the numbers beside it cannot show is whether it is the
   * rectangle on SCREEN. Every check on the data side passes — a window loses no vertex at any
   * zoom, the sample bins cell-for-cell against the predicate — so a frame with a straight-edged
   * black region is a frame whose points cover a different rectangle than the one being looked at,
   * and this is the number that says so.
   */
  box: { x: number; y: number; w: number; h: number } | null;
  /**
   * The level `matched` was counted at, and therefore the one that says whether a written `l{k}/`
   * answered: it can only exceed 0 when one did. The door used to carry a `read` field beside it
   * saying the same thing in words, which is a field that can disagree with the number next to it.
   */
  matchedAt: number;
  /** Vertices the rectangle holds at level 0, before the level decimated it, and vertices drawn. */
  matched: number;
  marks: number;
  /**
   * Far ends positioned but never painted — real vertices out of tiles already read.
   *
   * **Not "outside the window", which is what this said and is measurably wrong.** An anchor is a
   * vertex the answer did not *draw*, and there are two ways to be one: outside the rectangle, or
   * inside it and not at this level. Both are ends an edge needs and neither is a request.
   */
  anchors: number;
  links: number;
  /** Wall clock for the answer. */
  ms: number;
}

export interface CorpusSourceOptions {
  /**
   * The corpus, open — or opening. A promise is accepted because `openCorpus` is asynchronous and a
   * `BoundedSource` is not: the query loop builds one synchronously and calls it later.
   */
  corpus: Corpus | Promise<Corpus>;
  /**
   * The per-tile boxes, read from the Parquet footer once — **kept for `extent()` alone.**
   *
   * `Corpus.extent` answers the same question off the same footers, but it is a `Promise` whose
   * first call is a `parquet_metadata` sweep. `extent()` is what the camera frames itself with
   * before it asks for anything, so taking it from the door would put a round trip between opening
   * the corpus and the first paint. The panel around this source has already bought these with an
   * explicit button; this reads their union and issues nothing.
   */
  boxes: readonly TileBox[];
  /** Which vertex type to draw. Defaults to the first the index names. */
  vertexType?: string;
  /** Told what each answer cost, so the panel can print it beside the picture. */
  onCost?(cost: SliceCost): void;
  /**
   * How many slots the categorical palette has — `categoricalCapacity` of the host element.
   *
   * Asked for rather than assumed, because it is a fact about the THEME and this file cannot read a
   * stylesheet: `@kanzo-tech/ui` publishes it as `--chart-capacity` and its own themes disagree (7
   * in some, 8 in others). A number, so nothing here imports a design system.
   */
  slots?: number;
}

const EMPTY: Slice = {
  n: 0,
  marks: 0,
  vertices: new BigUint64Array(0),
  positions: new Float32Array(0),
  links: new Float32Array(0),
  categories: new Uint16Array(0),
};

/**
 * A hair past a coordinate, because `Corpus.view`'s box predicate is **half-open** on the far edge.
 *
 * `x >= x0 AND x < x0 + w` is what the door writes, and the extent's maximum IS a vertex — the one
 * that defines it — so clamping an open edge to `extent.xhi` exactly drops it and «the whole corpus»
 * comes back short of the whole corpus. The hair is `1e-6` **relative** and not `1e-9` because `x`
 * is a float32 column whose footer statistic arrives through a decimal string: the number held here
 * and the number compared there differ by up to one ulp, `1.2e-7` relative, which swallowed a
 * `1e-9` nudge whole and lost the 17 vertices on the two far edges of the bench corpus. Over-reaching
 * an OPEN edge costs nothing — it means everything, and there is nothing outside the extent to let in.
 */
const past = (value: number): number => value + (Math.abs(value) + 1) * 1e-6;

/**
 * The camera's rectangle as a finite `Box`, or `null` for a rectangle that holds nothing.
 *
 * **A viewport can be `±Infinity` and a `Box` cannot.** `shouldSlice` answers `false` for a graph
 * that fits and the loop then asks for everything with open edges. An open edge is clamped against
 * the corpus's own extent, which is the same rectangle in numbers the addressing can quantise:
 * interpolated, `Infinity` is not a comparison at all — DuckDB reads it as a column name — and the
 * arithmetic that turns a box into Morton codes has nothing to divide by.
 *
 * `null` is the degenerate rectangle a zero-sized canvas produces, where `xMin > xMax`. The honest
 * answer there is nothing rather than everything, and it costs no query to give.
 */
function boxFor(view: Viewport, extent: Rect | null): Box | null {
  if (extent === null || view.xMin > view.xMax || view.yMin > view.yMax) return null;
  const x = Number.isFinite(view.xMin) ? view.xMin : extent.xlo;
  const y = Number.isFinite(view.yMin) ? view.yMin : extent.ylo;
  const xMax = Number.isFinite(view.xMax) ? view.xMax : past(extent.xhi);
  const yMax = Number.isFinite(view.yMax) ? view.yMax : past(extent.yhi);
  return x > xMax || y > yMax ? null : { x, y, w: xMax - x, h: yMax - y };
}

/**
 * Which column colours a point — the request's, never the source's.
 *
 * A `fill` starting with `var(` or `#` is a COLOUR handed through the same field, and naming it as a
 * column asks DuckDB for `"#4c78a8"`. Omitted or a colour, `cluster_id`: the layout pass writes it
 * into every corpus, so it is the one column this reader can promise exists.
 */
function column(fill: string | undefined): string {
  return fill && !fill.startsWith('var(') && !fill.startsWith('#') ? fill : 'cluster_id';
}

/**
 * A `Frame` as the parallel arrays the renderer uploads — two conversions and one fold.
 *
 * `positions` passes through untouched: the door already answers in a `Float32Array`, marks first
 * and anchors after, which is the layout a `Slice` asks for.
 */
function sliceOf(view: Frame, typeIndex: number, slots: number): Slice {
  const rows = view.denseIds.length;
  const vertices = new BigUint64Array(rows);
  const categories = new Uint16Array(rows);
  for (let i = 0; i < rows; i += 1) {
    // `denseIds` is in this type's OWN numbering — a `dense_id` is unique within one vertex type and
    // repeats across a union of two — so the pair is completed here and nowhere else.
    vertices[i] = vertexId(typeIndex, Number(view.denseIds[i]!)) as bigint;
    // Folded into the palette's slots, and NOT ranked: an ordinal has to mean the same colour after
    // a pan, and a rank over the sample is renumbered by every camera move. A remainder is a
    // function of the community alone, so it survives one.
    //
    // **The fold is what makes the picture show communities at all.** `categoricalColor` answers
    // `var(--muted-foreground)` for any ordinal at or past the palette's capacity, and this corpus
    // carries 128 communities against a capacity of 8 — so 937,496 of a million vertices came back
    // one grey, which is what a pixel read of the canvas measured before this line existed. Two
    // communities sharing a slot is what eight slots MEANS; a million sharing one has given up.
    categories[i] = ((view.categories[i]! % slots) + slots) % slots;
  }
  // Row-index pairs: `Uint32Array` there, `Float32Array` here. The contract is the renderer's.
  const links = new Float32Array(view.links.length);
  for (let i = 0; i < view.links.length; i += 1) links[i] = view.links[i]!;
  return { n: view.matched, marks: view.marks, vertices, positions: view.positions, links, categories };
}

/**
 * A fossil corpus, as something a renderer can pan across.
 *
 * `total()` and `extent()` answer with no query at all, which is what lets the canvas frame the data
 * before it asks anything — the difference, measured on kanzo's side, between a first paint of the
 * corpus and a first paint of empty space with the corpus in one corner of it.
 */
export function corpusSource(options: CorpusSourceOptions): BoundedSource {
  const { boxes, onCost, slots = 8, vertexType } = options;
  const extent = extentOf(boxes);

  /**
   * The corpus and the three facts read off it, awaited once.
   *
   * The type's ordinal is what completes an identity, and the declaration order in `graph.graph.yml`
   * is the only ordering there is — so it is the one used, and a corpus of one type is `0` because
   * it is first rather than because zero is a default. `count` is off `Corpus.types`, which is a
   * property and not a call: it was read while the corpus was opening.
   */
  let opened: Promise<{ corpus: Corpus; type: string; typeIndex: number; count: number }> | null = null;
  const open = () => {
    opened ??= (async () => {
      const corpus = await options.corpus;
      const address = corpus.addressing.vertexType(vertexType);
      const declared = corpus.types.vertices.find((v) => v.type === address.type);
      return {
        corpus,
        type: address.type,
        typeIndex: Math.max(0, corpus.addressing.types.indexOf(address)),
        count: Number(declared?.count ?? address.count ?? 0n),
      };
    })();
    return opened;
  };

  return {
    /** How many vertices there are — **off the manifest, with no query.** */
    total: async () => (await open()).count,

    /** The rectangle the corpus occupies — **off the footer this source was handed, no query.** */
    extent: async (): Promise<Viewport> =>
      extent
        ? { xMin: extent.xlo, yMin: extent.ylo, xMax: extent.xhi, yMax: extent.yhi }
        : { xMin: 0, yMin: 0, xMax: 0, yMax: 0 },

    /**
     * No `explore`, and the absence is the statement.
     *
     * A rectangle is a map question and any relation with a spatial predicate answers it. A
     * neighbourhood is the graph question and needs adjacency held open. `useQueryLoop` narrows with
     * `"explore" in source`, so asking this source for one is a compile error rather than a promise
     * that rejects.
     */
    async slice(request: SliceRequest): Promise<Slice> {
      const { fill, limit = BOUNDED_DEFAULTS.limit, perPixel, pinned, view } = request;
      const started = performance.now();
      const { corpus, type, typeIndex } = await open();

      const box = boxFor(view, extent);
      const cost = (over: Partial<SliceCost>): void =>
        onCost?.({
          tiles: 0,
          ofTiles: boxes.length,
          requests: 0,
          bytes: 0,
          box: null,
          matchedAt: 0,
          matched: 0,
          marks: 0,
          anchors: 0,
          links: 0,
          ...over,
          ms: performance.now() - started,
        });
      if (box === null) {
        cost({});
        return EMPTY;
      }

      // Only this type's own pins: a `dense_id` from another type names the wrong row here rather
      // than none. The door counts their tiles onto the ledger itself and exempts them from the
      // level, because a pin is one `dense_id` and an odd one is a multiple of no stride above 1.
      const pins = (pinned ?? []).filter((v) => typeOf(v) === typeIndex).map(denseOf);

      // The renderer's floor is in PIXELS and `minLinkLength` is in the corpus's own units: a
      // renderer with `p` corpus units per pixel and a three-pixel floor passes `3p`. Nothing inside
      // `@fossil-lang/corpus` knows what a pixel is, and this is where the app says so.
      const minLinkLength =
        perPixel !== undefined && Number.isFinite(perPixel) && perPixel > 0
          ? BOUNDED_DEFAULTS.minLinkPixels * perPixel
          : 0;

      // **The renderer's budget, converted, and NOT the canvas.** The door takes pixels and
      // spends one mark per pixel, so `limit` marks is the square that holds them.
      //
      // `perPixel` makes the real canvas derivable — corpus units per pixel, so the rectangle's
      // width over it is the width in pixels — and asking for that instead is exactly the
      // reference viewer's own direction. It is not this app's call. Measured on the bench corpus
      // at a million vertices: the canvas is about 920×400, so deriving from it asks for 368,000
      // marks against `limit`'s 20,000, which drops the frame from level 3 to level 1 and draws
      // **250,000 points where 15,625 were asked for**. The result is a visibly laggy canvas and a
      // solid smear, because `BOUNDED_DEFAULTS.limit` is 20,000 for a stated reason — above about
      // fifty thousand points a live layout stops being comfortable.
      //
      // So the budget stays the renderer's until the renderer raises it, which is where that
      // decision belongs.
      const side = Math.max(1, Math.sqrt(limit));
      const pixels = { w: side, h: side };

      /**
       * **The pyramid answers this, and the condition that used to stop it has been met.**
       *
       * What stood here was an argument that it could not. The library would read a written
       * `l{k}/` only for a view that asked for no links, so a frame that wanted lines opened the
       * payload and `cost.matchedAt` stayed at `0`. Dropping the links to get the cheap read was
       * tried and rejected on the screen rather than on the ledger, and that half still holds — the
       * same rectangle, the two ways:
       *
       * | | marks | anchors | links | points drawn |
       * | --- | --- | --- | --- | --- |
       * | vertex level alone | 15,625 | 0 | 0 | **15,625** |
       * | the picture | 15,625 | 60,136 | 62,024 | **75,761** |
       *
       * The anchors are not fog. They are 4× the marks, real vertices at real coordinates, and
       * they are what fills the frame; a view without them is a sparse cloud with holes rather
       * than a cheaper version of the same picture.
       *
       * **That paragraph named its own reversal condition and the condition arrived.** It said:
       * *what would change it is a way to keep the far ends without opening the payload for them —
       * an edge representation a level can carry that is not a synthetic edge.* A level row of a
       * RELATION carries both endpoints' coordinates
       * (`crates/fossil-sinks/src/manifest.rs, Projection`), which is exactly that and is not
       * synthetic: the far end is the vertex, at the position the payload gives it. `Corpus.frame`
       * reads it, and `matchedAt` reports the level.
       *
       * Measured over the bench corpus by `scripts/measure-frame.mjs`, ten camera rectangles:
       * **every frame came back at level 1, 2 or 3, and every one of them asked for links** — 322
       * to 62,024 of them. Not one at level 0. The claim that used to be here was two commits
       * stale, and nothing in this file would have shown it: `matchedAt` is reported onto a ledger
       * and never read back.
       */
      const answer = await frame(corpus, {
        ...box,
        type,
        pixels,
        fill: column(fill),
        pinned: pins,
        minLinkLength,
      });

      cost({
        ...answer.cost,
        box: { x: box.x, y: box.y, w: box.w, h: box.h },
        matchedAt: answer.matchedAt,
        matched: answer.matched,
        marks: answer.marks,
        anchors: answer.positions.length / 2 - answer.marks,
        links: answer.links.length / 2,
      });
      return sliceOf(answer, typeIndex, slots);
    },
  };
}

/** Re-exported so a caller naming an identity does not have to import two packages for one pair. */
export { vertexId, type VertexId };
