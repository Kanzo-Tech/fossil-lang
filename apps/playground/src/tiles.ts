/**
 * A `BoundedSource` over a fossil corpus — the other end of a seam both sides had already built.
 *
 * ## The coincidence that is not one
 *
 * `crates/fossil-graph/src/lib.rs` says the camera is **addressed, not queried**: the `viewport`
 * verb was dropped, and what replaced it is a tile fetched by a URL a reader computes. From the
 * other side, `@kanzo-tech/graph`'s `duck-source.ts` says the same sentence about the same deleted
 * verb — «what replaces it is a tile brought by a computed URL, which is another source». This
 * file is that other source: fossil's addressing on one side, kanzo's `BoundedSource` on the
 * other, and nothing in between but a `WHERE` clause on `dense_id`.
 *
 * ## Why it is written here and not imported
 *
 * `@kanzo-tech/graph/duckdb` ships an `openCorpus` that reads a fossil corpus, and it is **not**
 * what this app calls, for a reason that is measurable rather than stylistic: it addresses
 * `<prefix>chunk{k}.parquet` and `<prefix>by_source/tile{k}.parquet`, which is `container: files`.
 * fossil's writer emits `container: rowgroups` — ONE `tiles.parquet` per payload set, where row
 * group *k* IS tile *k* — so every URL that reader composes against this corpus is a 404. It also
 * re-derives the addressing from a hand-written line scan over six manifest keys, which is exactly
 * the arithmetic `@fossil-lang/corpus/address` publishes so that nobody has to; its own comment
 * records what that cost the last time — a copied `chunk_size` «went stale and read a fraction of
 * a corpus in silence for as long as it did».
 *
 * So the split is: **kanzo owns the contract and the renderer, fossil owns the addressing.** This
 * file is the twenty lines where they meet, and it imports `resolveCorpus` rather than restating
 * any of it. Should `openCorpus` learn `rowgroups` — better, should it address through
 * `@fossil-lang/corpus/address` — this file becomes a call to it and the argument reverses.
 *
 * ## Why no Mosaic
 *
 * `openCorpus` and `duckBoundedSource` are clients of a Mosaic `Coordinator`, which is how a graph
 * ends up inside the same crossfilter as the charts beside it. There are no charts beside this
 * one. `BoundedSource` is one method, the app already owns a DuckDB connection, and adding
 * `@uwdata/mosaic-core`, `@uwdata/mosaic-sql` and `@kanzo-tech/mosaic` to create a coordinator
 * with one client in it would buy a crossfilter with nothing to cross. The contract invites this
 * explicitly: «a source is anything that can answer that question … the wiring between a
 * particular source and this contract belongs at the call site.»
 *
 * ## What a camera move costs
 *
 * Nothing that is not a byte of the tiles the window touches. `extent()` and `total()` are
 * answered from the footer and the manifest, so the opening frame issues no query at all; a pan
 * issues two, both restricted to `dense_id` ranges the reader computed from boxes it already
 * holds. There is deliberately no request between the camera moving and a URL being computable —
 * the moment there is one, this is the `viewport` verb again.
 */
import type { CorpusAddressing } from '@fossil-lang/corpus/address';
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

import type { QueryRow } from './duckdb.js';
import { extentOf, runsOf, selectTiles, type Rect, type Run, type TileBox } from './stream.js';

/** What one answer cost, in the terms the panel beside the canvas is already reporting. */
export interface SliceCost {
  /** Tiles the window's rectangle selected. */
  tiles: number;
  /** Tiles in the corpus. */
  ofTiles: number;
  /**
   * Maximal runs of adjacent tiles — what the same window would cost in `Range` requests if this
   * thread were fetching the bytes rather than the engine.
   *
   * **Derived, and it says so.** DuckDB issues the real requests from inside its Worker, where
   * this thread cannot weigh them; the ledger above the canvas weighs its own. This number is the
   * footer's arithmetic over the same tiles, which is the thing the ledger checks against bytes.
   */
  runs: number;
  /** What those runs add up to, from the footer's per-tile compressed sizes. */
  bytes: number;
  /** Vertices the rectangle matched, before `limit` sampled it. */
  matched: number;
  /** Vertices drawn. */
  marks: number;
  /**
   * Far ends positioned but never painted — real vertices out of tiles already read.
   *
   * **Not "outside the window", which is what this said and is measurably wrong.** An anchor is a
   * vertex the answer did not *draw*, and there are two ways to be one: outside the rectangle, or
   * inside it and not in the sample. Both are ends an edge needs and neither is a request —
   * `verify-canvas.mjs` counts 420 of the second kind in a 10% window and the assertion that
   * expected none of them is what caught the wording.
   */
  anchors: number;
  links: number;
  /** Wall clock for the two queries. */
  ms: number;
}

export interface CorpusSourceOptions {
  /** The corpus, resolved to addresses. Every URL below comes from here and from nowhere else. */
  addressing: CorpusAddressing;
  /**
   * The per-tile boxes, read from the Parquet footer once.
   *
   * Passed in rather than read here, because the panel around this source exists to show what the
   * footer costs and buys it with an explicit button. A source that read it lazily would be tidier
   * and would hide the one step on this path that needs an engine.
   */
  boxes: readonly TileBox[];
  /** Which vertex type to draw. Defaults to the first the index names. */
  vertexType?: string;
  /**
   * Make a name in DuckDB's filesystem point at a URL. The app's `duckdb.registerUrl`.
   *
   * A name rather than the URL itself, because `read_parquet('https://…')` re-negotiates the
   * whole HTTP file for every query, and a registered name is where duckdb-wasm keeps what it has
   * already read — which is what makes the footer bought once rather than bought per pan.
   */
  register(name: string, url: string): Promise<void>;
  query(sql: string): Promise<QueryRow[]>;
  /** Told what each answer cost, so the panel can print it beside the picture. */
  onCost?(cost: SliceCost): void;
  /**
   * How many slots the categorical palette has — `categoricalCapacity` of the host element.
   *
   * Asked for rather than assumed, because it is a fact about the THEME and this file cannot read
   * a stylesheet: `@kanzo-tech/ui` publishes it as `--chart-capacity`, and its own themes disagree
   * (7 in some, 8 in others). Passed as a number so nothing here imports a design system.
   */
  slots?: number;
}

/** `bigint` from one host and `number` from another — the graph package's contract says so. */
const num = (value: unknown): number => (typeof value === 'bigint' ? Number(value) : Number(value));

/** A single-quoted SQL string literal. */
const lit = (value: string) => `'${value.replace(/'/g, "''")}'`;

/**
 * A run of tiles as an interval of `dense_id`, which is the whole of why this is cheap.
 *
 * A tile is `chunkSize` consecutive dense ids, and `dense_id` ascends with the Morton code of the
 * vertex's position — the id space IS the spatial order. So the rectangle a camera is over becomes
 * a handful of ranges over one integer column, and Parquet's own per-row-group statistics on that
 * column are what stop the engine opening the other 228 row groups. The reader states which tiles
 * it wants in the id space; the format does the rest.
 */
function rangeSql(column: string, runs: readonly Run[], chunkSize: number): string {
  if (runs.length === 0) return 'FALSE';
  return runs
    .map((run) => `${column} BETWEEN ${run.first * chunkSize} AND ${(run.last + 1) * chunkSize - 1}`)
    .join(' OR ');
}

/**
 * The rectangle, as a predicate — and an unbounded rectangle as no predicate at all.
 *
 * `shouldSlice` answers `false` for a graph that fits, and the query loop then asks for everything:
 * a viewport whose edges are `±Infinity`. Interpolated, `x >= -Infinity` is not a comparison —
 * DuckDB parses `Infinity` as a column name and fails with `Referenced column "Infinity" not
 * found`. An open edge therefore contributes no clause, which is also the right plan: a query that
 * wants every row has nothing to prune.
 */
function bboxSql(view: Viewport): string {
  const bounds: [string, number, string][] = [
    ['x', view.xMin, '>='],
    ['x', view.xMax, '<='],
    ['y', view.yMin, '>='],
    ['y', view.yMax, '<='],
  ];
  const clauses = bounds
    .filter(([, value]) => Number.isFinite(value))
    .map(([column, value, op]) => `${column} ${op} ${value}`);
  return clauses.length > 0 ? clauses.join(' AND ') : 'TRUE';
}

/**
 * The tiles a rectangle touches, plus the tiles the pins live in.
 *
 * `selectTiles` intersects boxes, and `±Infinity` intersects everything — but only if the boxes
 * are finite, which they are. The empty branch is for the degenerate rectangle a zero-sized canvas
 * produces, where `xMin > xMax` and the honest answer is no tiles rather than all of them.
 *
 * **The pins widen the selection, and that is a fetch this reader owes rather than one it avoids.**
 * A pin is a vertex the caller has taken hold of, and its coordinate is wherever the layout put it
 * — usually nowhere near the rectangle. Left out of the tile set it is left out of `held`, and the
 * disjunct that is supposed to bring it back has nothing to match against: a pinned vertex was
 * silently dropped by the one clause written to keep it. Counting its tile here is what keeps the
 * ledger beside the canvas describing the bytes the query actually opens, which is the difference
 * between a cost this panel reports and a cost it estimates.
 */
function tilesFor(
  boxes: readonly TileBox[],
  view: Viewport,
  pinned: readonly number[],
  chunkSize: number,
): TileBox[] {
  const open = view.xMin <= view.xMax && view.yMin <= view.yMax;
  const rect: Rect = { xlo: view.xMin, xhi: view.xMax, ylo: view.yMin, yhi: view.yMax };
  const chosen = open ? selectTiles(boxes, rect) : [];
  if (pinned.length === 0) return chosen;
  const have = new Set(chosen.map((t) => t.tile));
  const want = new Set(pinned.map((dense) => Math.floor(dense / chunkSize)));
  return chosen.concat(boxes.filter((b) => want.has(b.tile) && !have.has(b.tile)));
}

/**
 * A fossil corpus, as something a renderer can pan across.
 *
 * The tile boxes and the manifest answer `extent()` and `total()` with no query at all, which is
 * what lets the canvas frame the data before it asks anything — the difference, measured on
 * kanzo's side, between a first paint of the corpus and a first paint of empty space with the
 * corpus in one corner of it.
 */
export function corpusSource(options: CorpusSourceOptions): BoundedSource {
  const { addressing, boxes, onCost, query, register, slots = 8, vertexType } = options;

  const type = addressing.vertexType(vertexType);
  const { chunkSize } = type;
  /**
   * The type's ordinal, which is what completes an identity.
   *
   * A `dense_id` numbers within ONE vertex type, so a union of two types repeats every value —
   * `vertexId(type, dense)` is the pair, and this is the half the corpus does not write down. The
   * declaration order in `graph.graph.yml` is the only ordering there is, so it is the one used,
   * and a corpus of one type is `0` because it is first rather than because zero is a default.
   */
  const typeIndex = Math.max(0, addressing.types.indexOf(type));

  /**
   * The edge relation whose source is this vertex type, in its `src` orientation — or nothing.
   *
   * `by_source` is tiled by `src_dense` with the same `chunk_size` as the vertices, which is what
   * the manifest asserts and `resolveCorpus` checks; that is what lets ONE set of tile numbers
   * address both relations. A corpus that publishes no `src` adjacency draws points and no links,
   * which is a legitimate picture and not a failure.
   */
  const edge = addressing.incident(type.type).find((e) => e.srcType === type.type);
  const adjacency = edge?.adjacency('src') ?? null;

  /** The names DuckDB knows the two payloads by. Registered once, on the first slice. */
  const VERTEX = `corpus-${type.type}-tiles.parquet`;
  const EDGE = `corpus-${type.type}-by-source.parquet`;

  let registered: Promise<void> | null = null;
  const ready = () => {
    registered ??= (async () => {
      await register(VERTEX, type.tileUrl(0));
      if (adjacency) await register(EDGE, adjacency.tileUrl(0));
    })();
    return registered;
  };

  const extent = extentOf(boxes);

  return {
    /**
     * How many vertices there are — **off the manifest, with no query.**
     *
     * The loop asks this first, and under `limit` it takes one slice covering everything and never
     * asks again. So this number decides whether the graph is panned on the GPU for free or
     * re-queried per camera move, and answering it costs a field the corpus already declared.
     */
    total: () => Promise.resolve(Number(type.count ?? 0n)),

    /**
     * The rectangle the corpus occupies — **off the footer, with no query.**
     *
     * The boxes were read to answer "which tiles does this rectangle touch"; their union is the
     * extent, for free. `null` only where there are no tiles at all, and a camera framed on a
     * degenerate box is better than one framed on `±Infinity`.
     */
    extent: () =>
      Promise.resolve<Viewport>(
        extent
          ? { xMin: extent.xlo, yMin: extent.ylo, xMax: extent.xhi, yMax: extent.yhi }
          : { xMin: 0, yMin: 0, xMax: 0, yMax: 0 },
      ),

    /**
     * No `explore`, and the absence is the statement.
     *
     * A rectangle is a map question and any relation with a spatial predicate answers it. A
     * neighbourhood is the graph question and needs adjacency held open — here it would be a
     * recursive join over an edge relation addressed one tile at a time, which is the unbounded
     * pattern wearing a bounded interface. `useQueryLoop` narrows with `"explore" in source`, so
     * asking this source for one is a compile error rather than a promise that rejects.
     */
    async slice(request: SliceRequest): Promise<Slice> {
      const { fill, limit = BOUNDED_DEFAULTS.limit, perPixel, pinned, view } = request;
      const started = performance.now();
      await ready();

      /**
       * A pinned vertex rides in the predicate, not in a second query.
       *
       * Its drawn position is a view-local overlay on a coordinate that never moved, so the
       * rectangle cannot find it where the reader dropped it. As a disjunct it is still inside the
       * one numbering, which is what keeps `links` speaking in buffer positions. Only this type's
       * own pins: a `dense_id` from another type names the wrong row here rather than none.
       *
       * Read BEFORE the tiles are chosen, because it is one of the two things that choose them:
       * a disjunct over rows that were never in `held` matches nothing at all.
       */
      const mine = (pinned ?? []).filter((v) => typeOf(v) === typeIndex).map(denseOf);
      const pins = mine.length > 0 ? ` OR dense_id IN (${mine.join(',')})` : '';

      const chosen = tilesFor(boxes, view, mine, chunkSize);
      const runs = runsOf(chosen);
      const held = rangeSql('dense_id', runs, chunkSize);
      const spatial = bboxSql(view);
      const inside = `((${spatial})${pins})`;

      /**
       * Which column colours a point — the request's, never the source's.
       *
       * Omitted, `cluster_id`: the layout pass writes it into every corpus, so it is the one
       * column this reader can promise exists. A name that is not a column of the payload is the
       * caller's error and DuckDB says which name it was, which is a better answer than a silent
       * fall back to a column nobody asked for.
       */
      const category = fill && !fill.startsWith('var(') && !fill.startsWith('#') ? fill : 'cluster_id';

      /**
       * The sample, and why it strides rather than truncating.
       *
       * `count(*) OVER ()` is evaluated over everything the `WHERE` kept and `LIMIT` applies after
       * it, so `matched` is what the window HOLDS and not what came back — the one honest thing a
       * bounded view owes its reader, and it costs no second scan. Striding on `dense_id` rather
       * than taking the front of the ordering is what makes the sample a picture of the window
       * instead of a picture of one corner of it: the ids ascend along the Morton curve, so every
       * s-th id is spatially stratified, where a prefix is a contiguous arc of the curve.
       *
       * **A pin is exempt from the stride, and `local` is numbered after the `LIMIT` and not
       * before it.** Both were wrong in the same clause. `s = ceil(matched / limit)` means one id
       * in `s` survives, and a pin is one id: `verify-canvas.mjs` asked for `dense_id 999999` in a
       * window whose stride was 2 and got nothing back, having already been given the tile. And
       * numbering with a window function over the pre-`LIMIT` set makes `local` a SUBSET of
       * `0..m-1` rather than a run of it, so the moment `LIMIT` binds the link indices address the
       * wrong rows and the anchors collide with unused ones. Ordering the sample is not the fix —
       * the numbering has to be over the rows that come back, which is what `kept` is for.
       */
      const stride = `greatest(1, CAST(ceil(matched / ${limit}.0) AS BIGINT))`;

      /**
       * `held` is every row of the tiles this window opened; `vis` is the sample that gets drawn.
       *
       * The two are not the same set and the gap is the point: a tile is `chunkSize` rows of a
       * Morton-ordered relation, so its box is wider than the rectangle, and the vertices just
       * outside the window are usually **in bytes the reader already fetched**. That is where the
       * far end of an edge leaving the window comes from — at no request, no extra query and no
       * extra byte.
       */
      const base = `WITH held AS (
    SELECT dense_id, x, y, ${category} AS cat
    FROM read_parquet(${lit(VERTEX)})
    WHERE ${held}
  ), pool AS (
    SELECT dense_id, x, y, cat, count(*) OVER () AS matched
    FROM held WHERE ${inside}
  ), kept AS (
    SELECT * FROM pool WHERE dense_id % ${stride} = 0${pins} LIMIT ${limit}
  ), vis AS (
    SELECT dense_id, x, y, cat, matched,
           (row_number() OVER (ORDER BY dense_id) - 1)::INTEGER AS local
    FROM kept
  )`;

      /**
       * The edges, and the far ends they need.
       *
       * `span` keeps an edge with at least one end drawn and both ends **positioned** — both in
       * `held`, which is the tiles that were read. An edge neither of whose ends is drawn is an
       * edge somewhere else, and drawing it would put ink outside the window the caller asked
       * about. `anchor` numbers the far ends past the marks, so `marks` stays a prefix length.
       *
       * The link floor is a row discard rather than a fade: an edge under three screen pixels is a
       * dot on top of two dots the point layer has already drawn, and dimming it happens after the
       * row has been joined, returned, uploaded and rasterised.
       */
      const floor =
        perPixel !== undefined && Number.isFinite(perPixel) && perPixel > 0
          ? BOUNDED_DEFAULTS.minLinkPixels * perPixel
          : 0;
      const long =
        floor > 0
          ? ` AND (a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) >= ${floor * floor}`
          : '';

      const withEdges = adjacency
        ? `${base}, span AS (
    SELECT sv.local AS src, tv.local AS dst, a.dense_id AS src_id, b.dense_id AS dst_id
    FROM read_parquet(${lit(EDGE)}) e
    JOIN held a ON a.dense_id = e.src_dense
    JOIN held b ON b.dense_id = e.dst_dense
    LEFT JOIN vis sv ON sv.dense_id = e.src_dense
    LEFT JOIN vis tv ON tv.dense_id = e.dst_dense
    WHERE (${rangeSql('e.src_dense', runs, chunkSize)})
      AND (sv.dense_id IS NOT NULL OR tv.dense_id IS NOT NULL)${long}
  ), anchor AS (
    SELECT h.dense_id, h.x, h.y,
           ((SELECT count(*) FROM vis) + row_number() OVER (ORDER BY h.dense_id) - 1)::INTEGER AS local
    FROM held h
    WHERE h.dense_id IN (SELECT src_id FROM span WHERE src IS NULL
                         UNION SELECT dst_id FROM span WHERE dst IS NULL)
  )`
        : base;

      /**
       * Marks then anchors, in one answer, because they are one buffer.
       *
       * `ORDER BY local` is load-bearing rather than tidy: `local` runs `0..marks-1` over the
       * sample and continues past it over the anchors, so ordering by it makes `marks` a prefix
       * length and leaves `matched` readable off row zero.
       */
      const pointsSql = adjacency
        ? `${withEdges}
  SELECT local, dense_id, x, y, cat, matched, TRUE AS mark FROM vis
  UNION ALL
  SELECT local, dense_id, x, y, 0, NULL::BIGINT, FALSE AS mark FROM anchor
  ORDER BY local`
        : `${withEdges}
  SELECT local, dense_id, x, y, cat, matched, TRUE AS mark FROM vis ORDER BY local`;

      const linksSql = adjacency
        ? `${withEdges}
  SELECT coalesce(sp.src, sa.local) AS src, coalesce(sp.dst, da.local) AS dst
  FROM span sp
  LEFT JOIN anchor sa ON sa.dense_id = sp.src_id
  LEFT JOIN anchor da ON da.dense_id = sp.dst_id`
        : null;

      const points = await query(pointsSql);
      const links = linksSql ? await query(linksSql) : [];

      const slice = assemble(points, links, typeIndex, slots);
      onCost?.({
        tiles: chosen.length,
        ofTiles: boxes.length,
        runs: runs.length,
        bytes: runs.reduce((a, run) => a + run.bytes, 0),
        matched: slice.n,
        marks: slice.marks,
        anchors: slice.positions.length / 2 - slice.marks,
        links: slice.links.length / 2,
        ms: performance.now() - started,
      });
      return slice;
    },
  };
}

/**
 * Rows to the parallel typed arrays the renderer takes.
 *
 * Plain objects rather than Arrow columns, because that is what this app's `query` hands back —
 * `@kanzo-tech/mosaic`'s `fillColumn` writes an Arrow column straight into a typed buffer with no
 * intermediate, and is the better route where an Arrow table is in hand. It is not, here: the
 * host's one capability is `sql => Record<string, unknown>[]`, which is the contract
 * `@fossil-lang/corpus` asks of a host and the one this app implements. At `limit` marks the loop
 * is twenty thousand iterations of four field reads.
 */
function assemble(
  points: readonly QueryRow[],
  links: readonly QueryRow[],
  typeIndex: number,
  slots: number,
): Slice {
  const rows = points.length;
  const positions = new Float32Array(rows * 2);
  const vertices = new BigUint64Array(rows);
  const categories = new Uint16Array(rows);

  let marks = 0;
  let matched = 0;
  for (let i = 0; i < rows; i++) {
    const row = points[i]!;
    positions[i * 2] = num(row.x);
    positions[i * 2 + 1] = num(row.y);
    vertices[i] = vertexId(typeIndex, num(row.dense_id)) as bigint;
    // Folded into the palette's slots, and NOT ranked: an ordinal has to mean the same colour
    // after a pan, and a rank over the sample is renumbered by every camera move. A remainder is
    // a function of the community alone, so it survives one.
    //
    // **The fold is what makes the picture show communities at all.** `categoricalColor` answers
    // `var(--muted-foreground)` for any ordinal at or past the palette's capacity, and this corpus
    // carries 128 communities against a capacity of 8 — so 937,496 of a million vertices came back
    // one grey, which is what a pixel read of the canvas measured before this line existed. Two
    // communities sharing a slot is what eight slots MEANS; a million points sharing one is a
    // scale that has silently given up.
    categories[i] = ((num(row.cat) % slots) + slots) % slots;
    // `mark` is TRUE down the sample and FALSE down the anchors, and the query orders by `local`,
    // so this is a prefix length rather than a count.
    if (row.mark === true || row.mark === 1) marks = i + 1;
    if (i === 0) matched = num(row.matched);
  }

  const edges = new Float32Array(links.length * 2);
  for (let i = 0; i < links.length; i++) {
    edges[i * 2] = num(links[i]!.src);
    edges[i * 2 + 1] = num(links[i]!.dst);
  }

  return {
    // What matched, before `limit` sampled it — read off row zero rather than counted, because it
    // is constant down the column and an empty answer has no row and no matches, which agree.
    n: matched,
    marks,
    vertices,
    positions,
    links: edges,
    categories,
  };
}

/** Re-exported so a caller naming an identity does not have to import two packages for one pair. */
export { vertexId, type VertexId };
