/**
 * `rows` and the tile selection `frame` reads through — the rectangle read over the footers, which
 * step 5 of `/docs/design/backend` deletes with both members. Held apart from `./scan.ts` for the
 * reason `./frame.ts` is: so the deletion is a file rather than a carving.
 */

import type { CorpusAddressing, Direction, Gap, VertexAddress } from './address.js';
import type { Answer, Edges, PlacedEdge } from './edges.js';
import { boxOf, type Box } from './expression.js';
import { vertexOf, type PlacedVertex } from './identity.js';
import type { QueryFn } from './query.js';
import { ascending, CorpusReadError, distinct, ident, list } from './sql.js';
import { runsOf, type TileBox, type TileManifest, type TileRun } from './tile-manifest.js';

/** What {@link Corpus.rows} takes. */
export interface RowsParams extends Box {
  /** The vertex type, defaulting to the first the index names. */
  type?: string;
  /**
   * Which orientations to read. Defaults to **both**, which is the answer that is complete for
   * incidence — the addressing's `tilesFor` defaults to `['src']` instead, because a picture pays
   * for nothing it cannot paint. The reference API's default is the honest one and the drawing path
   * opts down to it. Which RELATIONS a picture can paint is a separate question and a separate
   * call: `addressing.drawing`.
   */
  directions?: readonly Direction[];
}

/** What a rectangle read, entire. */
export interface RowsAnswer extends Answer {
  readonly type: string;
  readonly box: Box;
  /** The tiles the answer's vertices turned out to live in. Reported, never asked for. */
  readonly tiles: readonly bigint[];
  readonly vertices: readonly PlacedVertex[];
  readonly edges: readonly PlacedEdge[];
}

/**
 * The lines one `sampled` read draws, and where their coordinates come from.
 *
 * **{@link SampledLines.positioned} is the one fork a byte source does not dissolve**, because it
 * is a difference in what the bytes SAY rather than in where they are. A relation's level file
 * carries `src_x`…`dst_y` beside the two ids and draws its own far ends; a relation's projection
 * at `scale: 1` is the ADJACENCY, which carries the two ids and nothing else, so its far ends
 * have to be joined out of vertex tiles that were opened anyway. Same lines either way where
 * both ends are held — see `packages/corpus/integration/frame-levels.test.ts` — and two shapes of SQL
 * to get them.
 */
export interface SampledLines {
  /** The files the edge rows come out of. Empty when the caller asked for no links. */
  readonly urls: readonly string[];
  /** The columns those files are WEIGHED over — what the read projects, and never the file. */
  readonly columns: readonly string[];
  /** Whether an edge row positions both of its endpoints on its own. */
  readonly positioned: boolean;
  /** The relations incident to the drawn type that no line came out of, and why. */
  readonly undrawn: readonly Gap[];
}

/**
 * **One `sampled` read, resolved** — the seam that used to be a `viaLevel` boolean.
 *
 * A {@link CoarseSource} of `'sampled'` is real rows at real addresses, and this is the answer to
 * *which bytes serve them*. The payload and a written `l{k}/` return the same rows by
 * construction, so **everything that differs between them is here and nothing that differs
 * between them is a contract** — which footers, which tiles, how wide one is, which URLs, which
 * edge URLs, which columns the ledger counts, and whether a pin needs a tile of its own. One
 * implementation of `sampled` consumes this record; there is no second one, and there is no
 * third face for «the payload».
 *
 * The `'aggregated'` face has no record here because nothing on this side writes or reads a rung.
 * When one does it is a sibling of this, not a flag on it: its cells carry no `dense_id` to
 * address, its pins are `dense_id >> shift` rather than a read, and it has no `lines` at all.
 */
export interface SampledRead {
  /** Every tile the artefact that answered has — {@link FrameCost.ofTiles}. */
  readonly all: readonly TileBox[];
  /** The tiles of it this rectangle selects, in that artefact's own ordinals. */
  readonly tiles: readonly number[];
  /** Those tiles collapsed into maximal byte runs — what they cost in `Range` requests. */
  readonly runs: TileRun[];
  /**
   * **How wide one of its tiles is in `dense_id`.**
   *
   * The payload's `chunk_size` when the payload serves, and a projection's own tile times its
   * declared `scale` when a level file does — one row of it stands for that many ids, so its tile
   * of `chunkSize` rows spans that many times the ids. Both numbers come off the manifest and
   * neither is an exponent. Using the payload's over a level file is the bug this field exists
   * as: the range clause bounded tile 0 of `l6` at 8 ids where it holds 512, so the read came
   * back with one row and looked like a corpus rather than like a predicate.
   */
  readonly span: bigint;
  /** The files {@link SampledRead.tiles} name, distinct. */
  readonly urls: readonly string[];
  /**
   * The PAYLOAD tiles a pin needs opening for, and **empty when the bytes already carry every
   * id** — which is the honest form of the rule, not «is this a level read».
   *
   * A pin is one `dense_id` and an odd one is a multiple of no stride above 1, so a cache that
   * holds one row in `strideOf(k)` does not have it and its own tile is opened for it. Where the
   * payload serves, the pin is already in the selection and there is nothing to open twice —
   * which is why these are counted rather than discounted: under a cache the level tile was
   * genuinely opened and genuinely bought nothing.
   */
  readonly pinTiles: readonly number[];
  readonly pinRuns: TileRun[];
  readonly pinUrls: readonly string[];
  readonly lines: SampledLines;
  /** What {@link Frame.matchedAt} reports — and see {@link CoarseSource} for what it does not. */
  readonly matchedAt: number;
}

export interface Rectangles {
  selectedTiles(type: VertexAddress, box: Box): readonly number[];
  sampledRead(params: {
    readonly address: VertexAddress;
    readonly box: Box;
    readonly level: number;
    readonly stride: number;
    readonly pins: readonly bigint[];
    readonly wantLinks: boolean;
  }): Promise<SampledRead>;
  rows(params: RowsParams): Promise<RowsAnswer>;
}

export function rowsOf(reads: {
  readonly query: QueryFn;
  readonly addressing: CorpusAddressing;
  readonly payloadFiles: ReadonlyMap<string, readonly string[]>;
  readonly fieldsOf: (type: string) => readonly { readonly name: string }[];
  readonly has: (type: string, column: string) => boolean;
  readonly manifest: TileManifest;
  readonly edges: Edges;
}): Rectangles {
  const { query, addressing, payloadFiles, fieldsOf, has } = reads;
  const { tileBoxes, footers } = reads.manifest;
  const { readEdges } = reads.edges;

  /**
   * The tiles a rectangle can touch. Pure, and the box is half-open on the far edge exactly as
   * {@link boxOf} is, so a tile the `WHERE` would empty is never opened.
   */
  const intersecting = (all: readonly TileBox[], { x, y, w, h }: Box): readonly TileBox[] =>
    all.filter((b) => b.x1 >= x && b.x0 < x + w && b.y1 >= y && b.y0 < y + h);

  /**
   * The tiles a rectangle can touch, or the whole set when the footers do not bound it.
   *
   * A type whose every tile carries an `x`/`y` box is pruned; one where any tile's footer declares
   * none is read whole, which is the conservative answer and the one this had before the boxes were
   * cached at all. The comparison is against the TILE count and not the file count: under the
   * row-group container one file carries every tile.
   */
  const selectedTiles = (type: VertexAddress, box: Box): readonly number[] => {
    const all = footers.get(type.type);
    if (all === undefined) return [];
    return (BigInt(all.length) === type.tiles ? intersecting(all, box) : all).map((b) =>
      Number(b.tile),
    );
  };

  /**
   * Resolve a `sampled` read: **decide the cache once, and answer every byte question from it.**
   *
   * The cache lookup is the first three statements and the whole of it. Everything after them is
   * arithmetic off whichever artefact won, because a level file and a strided payload select the
   * same rows — so a caller of this cannot ask which one answered, and neither can `frame`.
   */
  const sampledRead = async (params: {
    readonly address: VertexAddress;
    readonly box: Box;
    readonly level: number;
    readonly stride: number;
    readonly pins: readonly bigint[];
    readonly wantLinks: boolean;
  }): Promise<SampledRead> => {
    const { address, box, level, stride, pins, wantLinks } = params;
    const chunk = BigInt(address.chunkSize);
    // The per-tile `x`/`y` boxes in the Parquet footers, which is the ONE index over which tiles a
    // rectangle touches — `footer-is-the-index`. Already read by `open` for every type with
    // geometry; awaited here for the one that has none cached.
    const all = await tileBoxes(address.type);

    /**
     * **Which artefact serves this stride** — and there is no arm here for «the payload».
     *
     * `address.ts` states it normatively: *the payload is not a special case — it is the projection
     * whose scale is one.* So stride 1 resolves to the payload through the same lookup every other
     * stride resolves through, and the `level === 0 ? null :` guard that used to stand here is
     * gone. It was never a face; it was this file disagreeing with the addressing vocabulary about
     * whether the payload is a projection, and paying for the disagreement with a second arm on
     * every question below. `null` now means one thing and one thing only: **nobody wrote a cache
     * at this stride**, so the payload is strided for it.
     */
    const levelSet = address.projection(stride);
    /**
     * **The relations this frame may draw**, out of the addressing and never out of a filter here.
     *
     * A frame is one type's `dense_id` space — every mark, every far end and both ends of every
     * line — so a relation it can draw is one whose two endpoints are that type. This asked
     * `tilesFor` for `['src']` instead, and that is the out-edge read: the out-edges of an `Author`
     * include the ones landing on a `Paper`, so the frame joined a `dst_dense` in `Paper`'s
     * numbering against `Author`'s. Two `dense_id` spaces are both dense from zero and both
     * `BIGINT`, so the join matched, and half the lines it drew joined vertices with nothing
     * between them. `ReadPlan::drawing` states the rule once, on the same side of the wasm boundary
     * every other address comes from, and `fossil-layout` states it for the writer.
     */
    const drawable = addressing.drawing(address.type);
    /**
     * The relations whose own level `k` is written, when every relation this frame draws is.
     *
     * **All or none, deliberately.** A frame drawing the edges of two relations out of one and the
     * payload out of the other would be reading two artefacts in one answer and reporting one
     * number for it.
     *
     * Source-aligned, always: a level of a relation is *which vertices are in it*, and the source
     * type's own pyramid is what says which. On a drawable relation the source type IS the drawn
     * type. A type that draws no relation has an empty set, which is a cache hit and not a
     * fallback.
     *
     * **Why asking for links is part of the cache lookup.** A vertex level holds the level's rows
     * and nothing else, so the far end of a mark-incident edge is not in it — and the camera keeps
     * an edge with ONE end drawn, not two. Measured on the 300,000-vertex bench corpus with the
     * app's own three-pixel floor (`crates/fossil-layout/tests/levels.rs`,
     * `what_the_pixel_floor_leaves_of_a_coarse_view`): a coarse view draws 51,254 edges and 46,571
     * far ends, of which a VERTEX level can position **405 — 0.79%**. So a vertex level alone is
     * not a cache of a view that asked for links; the PAIR is.
     */
    const edgeLevels = (() => {
      const sets = drawable.relations.map((e) => e.projection(stride, 'src'));
      return sets.every((s) => s !== null) ? (sets as NonNullable<(typeof sets)[number]>[]) : null;
    })();
    /**
     * **A cache HIT** — an artefact exists at this stride that carries what the answer needs.
     *
     * True at stride 1, where that artefact is the payload. Nothing past this line is allowed to
     * mean anything else by it, and in particular nothing past it may mean «a level answered»:
     * that is {@link SampledRead.matchedAt}, which is a cost and not a contract.
     */
    const cached = levelSet !== null && (!wantLinks || edgeLevels !== null);
    /**
     * **Does the artefact that serves carry EVERY id?**
     *
     * The honest form of what used to read «is this a level read», and the two stopped agreeing
     * the moment the payload became a projection like any other. What decides whether a pin needs
     * a tile of its own is not which artefact answered but whether that artefact is complete in
     * `dense_id`: a cache holding one row in `strideOf(k)` has no odd id in it, and the payload —
     * served as a hit at scale 1 or strided on a miss — has all of them.
     */
    const whole = !cached || levelSet!.scale === 1;
    /**
     * **Does an edge row of it position its own endpoints?**
     *
     * Scale, again, and not «is this a level read»: a relation's projection at `scale: 1` is the
     * ADJACENCY, which carries `src_dense` and `dst_dense` and no coordinates at all, while a
     * level of a relation carries `src_x`…`dst_y` beside them. Keying this on the hit rather than
     * on the scale is what made the level-0 guard load-bearing — it would have composed
     * `src_x` against an adjacency that has no such column.
     */
    const positioned = cached && edgeLevels !== null && edgeLevels.every((e) => e.scale > 1);

    const selected = new Set<number>(selectedTiles(address, box));
    // A pin's tile is COUNTED, and **only where this read is the one that brings it back.** Where
    // the artefact is `whole` there is a single read and the pin has to be inside the selection:
    // left out, the disjunct that returns it has nothing to match against, and the fetch it costs
    // would be missing from the ledger rather than absent from the read. Where it is not, there is
    // a SECOND read — `pinUrls` below — and adding the tile here as well counted one pin twice:
    // once mapped into `tiles` as a level tile, once in `pinTiles` as a payload one, and
    // `FrameCost.tiles` is their sum. That is the whole of «a pin costs two tiles, not one», which
    // `verify-canvas` has held red since it was measured. It is not a discount: the level tile was
    // genuinely opened and genuinely bought nothing. A pin the rectangle already selects is still
    // in `selected` from `selectedTiles`; one it does not select carries only rows the box
    // predicate drops.
    if (whole) for (const tile of address.tilesOf(pins)) selected.add(Number(tile));
    const payloadTiles = [...selected].sort((a, b) => a - b);

    /**
     * The same tiles, read off the cache — arithmetic, and **nothing new to address it with.**
     *
     * Level `k`'s tile `j` covers the `dense_id` range `[j·chunk·stride, (j+1)·chunk·stride)`, so a
     * run of payload tiles is a run of level tiles under the level's own shift. That is why the
     * manifest writes no per-level index: the footers that addressed the payload have already
     * addressed the level.
     */
    const cachedTilesOf = (tiles: readonly number[]): number[] => {
      const out = new Set<number>();
      for (const run of runsOf(tiles, new Map(all.map((b) => [Number(b.tile), b])))) {
        const lo = levelSet!.tileOf(BigInt(run.first) * chunk);
        const hi = levelSet!.tileOf(BigInt(run.last + 1) * chunk - 1n);
        for (let t = lo; t <= hi; t += 1n) out.add(Number(t));
      }
      return [...out].sort((a, b) => a - b);
    };

    const source = cached
      ? { all: await tileBoxes(address.type, level), tiles: cachedTilesOf(payloadTiles) }
      : { all, tiles: payloadTiles };
    const held = source.tiles;
    const runs = runsOf(held, new Map(source.all.map((b) => [Number(b.tile), b])));
    // A pin's tile is a PAYLOAD tile whichever artefact served the sample, so its cost is measured
    // against the payload's footers and added to the ledger the sample's own runs opened.
    const pinTiles = whole ? [] : [...new Set(address.tilesOf(pins).map(Number))].sort((a, b) => a - b);

    // **One composition for both shapes of the read**, off the projections of the relations this
    // frame may draw. The scale is the only difference: a hit reads the relation's own level set,
    // whose tiles carry the same ordinals the vertex level's do — both are `src_dense` shifted by
    // the source's shift plus `k`, from one plan — and a miss reads the adjacency, which is the
    // projection at scale 1. So the tiles already selected address either with no new arithmetic.
    //
    // The miss arm asked `tilesFor` for `['src']` and flattened its `edgeUrls`, which is where the
    // cross-type file got in: that answer is the out-edge set and it is right about it. A file of
    // another `dense_id` space now has no route to the query at all, rather than a predicate
    // downstream that would have to recognise it.
    const drawn = positioned
      ? edgeLevels!
      : drawable.relations.map((relation) => relation.adjacency('src')!);
    const edgeUrls = !wantLinks
      ? []
      : distinct(drawn.flatMap((set) => held.map((tile) => set.tileUrl(tile))));

    return {
      all: source.all,
      tiles: held,
      runs,
      span: cached ? BigInt(levelSet!.chunkSize) * BigInt(levelSet!.scale) : chunk,
      urls: distinct(
        cached
          ? held.map((tile) => levelSet!.tileUrl(tile))
          : held.map((tile) => address.tileUrl(tile)),
      ),
      pinTiles,
      pinRuns: runsOf(pinTiles, new Map(all.map((b) => [Number(b.tile), b]))),
      pinUrls: whole ? [] : distinct(address.tilesOf(pins).map((t) => address.tileUrl(t))),
      lines: {
        urls: edgeUrls,
        columns: positioned
          ? ['src_dense', 'dst_dense', 'src_x', 'src_y', 'dst_x', 'dst_y']
          : ['src_dense', 'dst_dense'],
        positioned,
        undrawn: drawable.undrawn,
      },
      matchedAt: cached ? level : 0,
    };
  };

  return {
    selectedTiles,
    sampledRead,

    /**
     * The vertices in a rectangle and the edges among them, entire.
     *
     * **Vertices are pruned by the engine and edges by the address, and the asymmetry is the whole
     * design.** A box over `x`/`y` is one conjunctive range, which is precisely what a Parquet
     * row-group box answers, so the tile selection is left to DuckDB's footer pruning and this
     * writes none of it. That is not a contradiction of the measured finding that *pruning cannot
     * be expressed as a predicate*: what was measured there is 179 disjoint `dense_id` ranges,
     * which cost 189 ms against 5 ms for no pruning at all because the engine evaluates them per
     * row. The `WHERE` here selects which rows come back; what selects which bytes are read is the
     * footer, and the Hilbert order is what keeps the boxes tight enough for it to matter.
     *
     * Edges have no such column. An adjacency tile is addressed by the *vertex* tile of the endpoint
     * it is ordered by, so the tiles are computed from the answer — `dense_id >> shift` over the
     * vertices that came back — and only those files are opened. Asking the whole relation instead
     * was measured at a million vertices and did not return in 45 seconds.
     *
     * **`complete` is about incidence.** With both orientations it is `true`: every edge touching a
     * vertex in the box is in the answer. With `['src']` it is `false` with a `not-requested` gap,
     * because every drawable edge has its source on screen, and the edges whose destination is
     * drawn and whose source is off it render identically to nothing.
     * An orientation the corpus does not publish is a `not-declared` gap, which is a different fact
     * and is not reported as the same one.
     *
     * What it does **not** report is truncation, because it does not truncate: there is no row cap,
     * and a box over a dense region returns everything in it. A caller that needs a bound puts it
     * in the box.
     */
    async rows(params) {
      const { type, directions = ['src', 'dst'], ...box } = params;
      const address = addressing.vertexType(type);
      if (!has(address.type, 'x') || !has(address.type, 'y')) {
        throw new CorpusReadError(
          `${address.type} carries no x/y, so no rectangle names any of it — its payload is ` +
            `${fieldsOf(address.type).map((f) => f.name).join(', ') || 'empty'}`,
        );
      }

      const all = await tileBoxes(address.type);
      // Every TILE carries a box, or the cache would have dropped it and this would read the whole
      // set — which is what it did before the cache existed, and is still the correct answer. The
      // comparison is against the tile count and not the file count: under the row-group container
      // one file carries every tile, and comparing files would have made a corpus with two or more
      // tiles look like one with boxes missing.
      const candidates = BigInt(all.length) === address.tiles
        ? distinct(intersecting(all, box).map((b) => address.tileUrl(b.tile)))
        : [...payloadFiles.get(address.type)!];
      const rows =
        candidates.length === 0
          ? []
          : await query(`SELECT * FROM read_parquet(${list(candidates)}) WHERE ${boxOf(box)}`);
      const vertices = rows.map((row) => vertexOf(address.type, row));
      const tiles = [...new Set(address.tilesOf(vertices.map((v) => v.denseId)))].sort(ascending);

      // The addressing decides which orientations apply and why one is missing; this only fetches.
      // Reproducing that rule here is how the two halves of a rectangle read drift apart.
      const plan = addressing.tilesFor({ type: address.type, tiles, directions });
      const edges =
        tiles.length === 0
          ? []
          : await readEdges(
              address.type,
              plan.edges,
              (adjacency) =>
                `${ident(adjacency.column)} IN (SELECT dense_id FROM ` +
                `read_parquet(${list(plan.vertexUrls)}) WHERE ${boxOf(box)})`,
            );

      return {
        type: address.type,
        box,
        tiles,
        vertices,
        edges,
        complete: plan.complete,
        gaps: plan.gaps,
      };
    },
  };
}
