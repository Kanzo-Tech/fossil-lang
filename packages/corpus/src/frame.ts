/**
 * `frame` — one rectangle at one resolution, composed as one query over the tiles a `sampled` read
 * resolved. It retires with the level files, into `scan` at a `z`.
 */

import { strideOf, type CorpusAddressing, type Gap } from './address.js';
import { boxOf, type Box } from './expression.js';
import type { QueryFn } from './query.js';
import type { Scan } from './scan.js';
import { ascending, CorpusReadError, distinct, floatOf, ident, idOf, list } from './sql.js';
import type { TileManifest } from './tile-manifest.js';
import { levelForCanvas, type Pixels } from './tile-matrix.js';
import { PAYLOAD_CATEGORICAL } from './vocabulary.generated.js';

/**
 * **Which kind of picture a coarse read comes back as** — and there are TWO of them, not three.
 *
 * A coarse answer has two faces because a coarse artefact has two kinds, and the difference
 * between them is what a caller cannot deduce and genuinely needs:
 *
 * - **`sampled`** — real rows, real addresses, real lines: a sample OF THE GRAPH. Every level of
 *   the pyramid is one, and so is the payload. A level *is* the predicate
 *   `dense_id % strideOf(k) == 0` over the payload, so a level file and a computed stride return
 *   **the same rows with the same `dense_id`s** — `stride 4 = l1`, `stride 16 = l2`,
 *   `stride 64 = l3`, measured identical to four decimals on total-variation fidelity over
 *   com-DBLP. Which of the two served the bytes is a **cache
 *   hit against a cache miss** and nothing else; it is invisible from out here on purpose, and
 *   `levelsOf`'s `written` is that cache's index rather than a second contract. The payload at
 *   stride 1 is `sampled` too — it is the sample that leaves everything in, which is why level 0
 *   needs no face of its own.
 * - **`aggregated`** — a rung, `cell/r{k}/`: synthetic summary cells carrying `count`, `mode` and
 *   `purity`, no real `dense_id` behind any of them, and **no drawable lines** —
 *   `/docs/design/cells` argues, measured, that no coarse quotient of a real graph is drawable.
 *   A summary OF THE FIELD, which is a different picture from a sample of the graph and not a
 *   second price for the same one. A pin in an `aggregated` answer draws as its own cell,
 *   `dense_id >> shift`: arithmetic, never a second read.
 *
 * **Nothing on this side writes or reads a rung yet**, so this names the second face and the
 * package implements the first. {@link Frame} does not carry it either: `matchedAt` still reports
 * a level, which is what the contract suite pins, and moving the wire from *what one cost* to
 * *which picture* is a decision separate from the seam that makes it sayable.
 */
export type CoarseSource = 'sampled' | 'aggregated';

/** What {@link Corpus.frame} takes — a rectangle, a resolution, and nothing about the session. */
export interface FrameParams extends Box {
  /** The vertex type, defaulting to the first the index names. */
  type?: string;
  /**
   * How big the picture is, which is what decides the level. **One mark per pixel**, and that is
   * the whole conversion — a factor below it would be a constant this package invented about
   * somebody else's renderer. Marks four pixels wide: pass the canvas divided by four.
   *
   * Required unless {@link FrameParams.level} names a level outright.
   */
  pixels?: Pixels;
  /**
   * A level, named rather than derived — **manual mode, and it is not a fallback.** *«The same
   * rectangle at the same level»* has to stay sayable or monotone refinement cannot be stated at
   * all, and a derived level is derived from a number a caller would have to invert to say it.
   * Given here it wins and `pixels` is not consulted.
   */
  level?: number;
  /**
   * Which column carries the categorical the caller will colour by.
   *
   * Defaults to the column `corpus.bnf` gives the `categorical` role — read from
   * `vocabulary.generated.ts` and not written here, because this door used to be the fourth place
   * one name was spelled out by hand and the generated one was the only machine-readable of the
   * four with no consumer.
   *
   * **The default is a convention, and a caller who has read the corpus outranks it.** The role
   * says what fossil's writer emits; a vertex type's `channels:` block says what *this* corpus
   * carries, and where the two disagree the corpus is right. This package cannot see that block —
   * the per-type manifests are parsed by `fossil-graph` in WASM, which does not surface it — so a
   * caller that has read it names the column here and it wins.
   */
  fill?: string;
  /**
   * Addresses that ride whatever the rectangle and the level select — a pinned vertex.
   *
   * They are exempt from the level, because a pin is one `dense_id` and an odd one is a multiple of
   * no stride above 1. Their tiles are counted on {@link FrameCost.tiles}, so a pin's fetch is on
   * the ledger rather than hidden in it.
   */
  pinned?: readonly (number | bigint)[];
  /** Whether to answer with the edges among the drawn set. Defaults to `true`. */
  links?: boolean;
  /**
   * The shortest edge worth a row, **in the corpus's own units and never in pixels** — which
   * {@link FrameParams.pixels} is, and the two do not meet: a canvas sizes the decimation, a
   * length floor measures the corpus. A renderer with `p` corpus units per pixel and a
   * three-pixel floor passes `3p`.
   */
  minLinkLength?: number;
}

/**
 * What one answer cost, in the terms a reader can check against a network tab.
 *
 * **Requests and bytes are the answer; tiles are how it was arrived at.** And there is no field
 * saying which artefact replied: the pyramid is complete, so the level that answered is
 * {@link Frame.matchedAt}, and whether it was written is `levelsOf` in `./address.ts` — a field
 * restating a derivation is a field that can disagree with it.
 */
export interface FrameCost {
  /** `Range` requests — maximal runs of tiles whose bytes abut, which is what a run costs. */
  readonly requests: number;
  /** Compressed bytes those requests hold, from the Parquet footers. */
  readonly bytes: number;
  /** Wall clock for the queries this answer issued. */
  readonly ms: number;
  /** The working: tiles the rectangle and the pins selected. */
  readonly tiles: number;
  /** The working: tiles the artefact that answered has in total. */
  readonly ofTiles: number;
}

/**
 * One picture at one resolution — **and it is a function of its arguments and nothing else.**
 *
 * No cap that moves with what the rectangle holds, no stride derived from a count, no state
 * between calls. `/docs/design/one-door` argues why that purity is the specification rather than a
 * property this happens to have, and `/docs/design/camera` measures what it buys.
 *
 * Parallel arrays rather than objects, because the consumer is a renderer that uploads them: at
 * twenty thousand marks a `PlacedVertex[]` is twenty thousand objects built to be read four fields
 * at a time and thrown away. {@link Corpus.rows} is the one that answers with rows.
 */
export interface Frame {
  readonly type: string;
  readonly box: Box;
  readonly level: number;
  /** `strideOf(level)`. */
  readonly stride: number;
  /**
   * How many vertices the rectangle holds **at {@link Frame.matchedAt}** — the denominator, and
   * the one number that says whether a coarse frame is a picture of the whole rectangle or of part
   * of it.
   */
  readonly matched: number;
  /**
   * The level {@link Frame.matched} counts at: `0` when the payload was strided, and
   * {@link Frame.level} when a written level answered — **which is also how a caller learns which
   * artefact replied**, since only a written `l{k}/` can report a level above zero.
   *
   * **A field rather than a footnote, because the honest answer is not always level 0.** `matched`
   * is `count(*)` over the rows that were read, and a level file holds one row in `strideOf(k)`, so
   * a read of `l{k}/` cannot count level 0 without opening the very bytes the pyramid exists to
   * avoid. Reporting `matched · strideOf(k)` instead would invent an estimate the type does not
   * admit to — that arithmetic is the caller's, and it is an estimate: the decimation is uniform
   * in `dense_id`, not in the rectangle.
   */
  readonly matchedAt: number;
  /**
   * `dense_id`s, marks first and then anchors. In this type's own numbering: a `dense_id` is unique
   * within one vertex type and repeats across a union of two, so a caller drawing more than one
   * type pairs these with {@link Frame.type} itself.
   */
  readonly denseIds: BigUint64Array;
  /** `x, y` per row, marks first and then anchors. */
  readonly positions: Float32Array;
  /** The `fill` column per row, as written. Only the first {@link Frame.marks} are meaningful. */
  readonly categories: Uint32Array;
  /**
   * How many of the rows are **drawn**. A prefix length rather than a count: the rows past it are
   * anchors — far ends the links need, at their real positions, out of tiles already opened.
   */
  readonly marks: number;
  /** Pairs of row indices into {@link Frame.positions}. At least one end of each is a mark. */
  readonly links: Uint32Array;
  /**
   * The relations incident to {@link Frame.type} that no line here came out of, and why.
   *
   * **A picture that quietly leaves a relation out is a picture of a graph that does not exist**,
   * and the reason matters: `other-space` is a relation whose far end is a different vertex type,
   * which one type's `dense_id` numbering cannot place — those edges are real and this is the wrong
   * question to ask for them, {@link Corpus.rows} being the right one. `not-declared` is a relation
   * the corpus publishes no source-aligned half of, so nothing addresses it.
   */
  readonly undrawn: readonly Gap[];
  readonly cost: FrameCost;
}

/**
 * The column {@link FrameParams.fill} falls back to — the payload's `categorical` role, which
 * `corpus.bnf` glosses *an ordinal the writer computed, for a reader to colour by*.
 *
 * One entry today and read as one on purpose: a frame carries ONE categorical per row, so a second
 * role column would need a second array on {@link Frame} before it could mean anything here.
 */
const CATEGORICAL = PAYLOAD_CATEGORICAL[0]!;

export function frameOf(reads: {
  readonly query: QueryFn;
  readonly addressing: CorpusAddressing;
  readonly fieldsOf: (type: string) => readonly { readonly name: string }[];
  readonly has: (type: string, column: string) => boolean;
  readonly manifest: TileManifest;
  readonly scan: Scan;
}): (params: FrameParams) => Promise<Frame> {
  const { query, addressing, fieldsOf, has } = reads;
  const { bytesOf, footers } = reads.manifest;
  const { sampledRead, selectedTiles } = reads.scan;

  return async (params) => {
    const started = Date.now();
    const {
      type,
      level: named,
      pixels,
      fill = CATEGORICAL,
      pinned = [],
      links: wantLinks = true,
      minLinkLength = 0,
      ...box
    } = params;
    const address = addressing.vertexType(type);
    if (!has(address.type, 'x') || !has(address.type, 'y')) {
      throw new CorpusReadError(
        `${address.type} carries no x/y, so no rectangle names any of it — its payload is ` +
          `${fieldsOf(address.type).map((f) => f.name).join(', ') || 'empty'}`,
      );
    }
    if (named !== undefined && (!Number.isInteger(named) || named < 0)) {
      throw new CorpusReadError(`a level is a non-negative integer; got ${String(named)}`);
    }
    const level =
      named ??
      levelForCanvas(
        address,
        footers.has(address.type) ? selectedTiles(address, box).length : null,
        pixels,
      );
    const stride = Number(strideOf(level));
    const chunk = BigInt(address.chunkSize);
    const pins = [...new Set(pinned.map((id) => BigInt(id)))].sort(ascending);

    /**
     * **Which bytes serve this sample** — one lookup, and every fork that used to hang off a
     * boolean is a field of what comes back.
     *
     * A level file and a computed stride return the same rows, so `frame` from here down cannot
     * tell which answered and does not try: it reads {@link SampledRead.urls},
     * {@link SampledRead.span}, {@link SampledRead.runs} and {@link SampledRead.lines} and
     * composes ONE query shape over them. See {@link CoarseSource}.
     */
    const read = await sampledRead({ address, box, level, stride, pins, wantLinks });
    const { runs, span, urls, pinRuns, pinUrls } = read;
    const held = read.tiles;
    // The vertex half, weighed over the four columns a view draws with rather than over the
    // whole tile — `runs` carries the tile's total and that total includes `subject`.
    //
    // Two calls and not one over the union, because the two sets are bounded by different
    // intervals: the selection's runs at whatever `span` the artefact that answered uses, and
    // the pins' own payload tiles at the payload's `chunk`. One call could only be bounded by
    // one of them, and under `rowgroups` an unbounded call weighs the file.
    const drawnColumns = distinct(['dense_id', 'x', 'y', fill]);
    const bytes =
      (await bytesOf(urls, drawnColumns, { column: 'dense_id', runs, span })) +
      (await bytesOf(pinUrls, drawnColumns, {
        column: 'dense_id',
        runs: pinRuns,
        span: chunk,
      }));

    const empty: Frame = {
      type: address.type,
      box,
      level,
      stride,
      matched: 0,
      denseIds: new BigUint64Array(0),
      positions: new Float32Array(0),
      categories: new Uint32Array(0),
      marks: 0,
      matchedAt: read.matchedAt,
      links: new Uint32Array(0),
      undrawn: read.lines.undrawn,
      cost: { requests: 0, bytes: 0, ms: 0, tiles: 0, ofTiles: read.all.length },
    };
    if (urls.length === 0) return empty;

    const ranges = (column: string): string =>
      runs
        .map(
          (run) =>
            `${column} BETWEEN ${BigInt(run.first) * span} AND ${BigInt(run.last + 1) * span - 1n}`,
        )
        .join(' OR ');
    const pinList = pins.length > 0 ? `dense_id IN (${pins.join(', ')})` : null;
    const strided = stride > 1 ? `dense_id % ${stride} = 0` : 'TRUE';

    /**
     * The rows the answer is computed over — and note what is NOT conditional on where they came
     * from: `pool` still applies `dense_id % strideOf(k) = 0` below, over a level file whose every row
     * already satisfies it. That is deliberate and it is the contract, executed rather than
     * asserted: a level file that disagreed with the predicate could change the BYTES this read
     * costs and could not change the rows it answers with.
     */
    const heldSql =
      `  SELECT dense_id, x, y, ${ident(fill)} AS cat FROM read_parquet(${list(urls)})\n` +
      `  WHERE ${ranges('dense_id')}\n` +
      (pinUrls.length === 0
        ? ''
        : `  UNION ALL\n  SELECT dense_id, x, y, ${ident(fill)} AS cat FROM ` +
          `read_parquet(${list(pinUrls)}) WHERE dense_id IN (${pins.join(', ')})\n`);
    const base =
      `WITH held AS (\n${heldSql}` +
      `), inrect AS (\n  SELECT * FROM held WHERE ${boxOf(box)}\n` +
      `), pool AS (\n  SELECT dense_id, x, y, cat FROM inrect WHERE ${strided}\n` +
      (pinList === null
        ? ''
        : `  UNION\n  SELECT dense_id, x, y, cat FROM held WHERE ${pinList}\n`) +
      `), vis AS (\n` +
      `  SELECT dense_id, x, y, cat, (SELECT count(*) FROM inrect) AS matched,\n` +
      `         (row_number() OVER (ORDER BY dense_id) - 1)::INTEGER AS local FROM pool\n)`;

    // Where the lines come from — resolved with the sample, because it is the same cache lookup:
    // a relation's level set is addressed by the tiles the vertex level already chose.
    const edgeUrls = read.lines.urls;

    /**
     * The far ends, and why they are in the same answer.
     *
     * `span` keeps an edge with at least one end drawn and BOTH ends positioned — both in `held`,
     * which is the tiles that were opened. An edge neither of whose ends is drawn is an edge
     * somewhere else. `anchor` numbers the far ends past the marks, so `marks` stays a prefix
     * length and a caller can slice rather than filter.
     */
    const floor = Number.isFinite(minLinkLength) && minLinkLength > 0 ? minLinkLength : 0;
    /**
     * The renderer's floor, over whichever pair of columns holds the two ends.
     *
     * Two callers and one expression: the payload path reads the ends off two joins of `held`,
     * the level path reads them off the edge row itself. Written once because a floor spelled
     * twice is a floor that drifts, and a view whose links obey a different threshold from the
     * one it reports is a view that cannot be compared with itself.
     */
    const lengthFloor = (ax: string, ay: string, bx: string, by: string): string =>
      floor > 0
        ? ` AND (${ax} - ${bx}) * (${ax} - ${bx}) + (${ay} - ${by}) * (${ay} - ${by}) >= ${floor * floor}`
        : '';
    const long = lengthFloor('a.x', 'a.y', 'b.x', 'b.y');
    /**
     * The self-drawing edge half: the lines and their ends out of one file.
     *
     * The shape a {@link SampledLines.positioned} source takes, which is a level of a relation
     * and never an adjacency. `links` keeps an edge with at least one end drawn, which is the
     * same rule the joined shape applies — and `anchor` places the other end from the edge row
     * rather than from a vertex tile, which is the whole difference. The length floor is applied
     * here as it is there, over coordinates the file already carries.
     */
    const selfDrawn =
      `${base}, links AS (\n` +
      `  SELECT src_dense, dst_dense, src_x, src_y, dst_x, dst_y\n` +
      `  FROM read_parquet(${list(edgeUrls)})\n` +
      `  WHERE (${ranges('src_dense')})${lengthFloor('src_x', 'src_y', 'dst_x', 'dst_y')}\n` +
      `    AND (src_dense IN (SELECT dense_id FROM vis) OR dst_dense IN (SELECT dense_id FROM vis))\n` +
      `), anchor AS (\n` +
      `  SELECT dense_id, x, y,\n` +
      `         ((SELECT count(*) FROM vis) + row_number() OVER (ORDER BY dense_id) - 1)::INTEGER AS local\n` +
      `  FROM (SELECT DISTINCT src_dense AS dense_id, src_x AS x, src_y AS y FROM links\n` +
      `         WHERE src_dense NOT IN (SELECT dense_id FROM vis)\n` +
      `        UNION\n` +
      `        SELECT DISTINCT dst_dense, dst_x, dst_y FROM links\n` +
      `         WHERE dst_dense NOT IN (SELECT dense_id FROM vis))\n)`;

    const withEdges =
      edgeUrls.length === 0
        ? base
        : read.lines.positioned
          ? selfDrawn
          : `${base}, span AS (\n` +
          `  SELECT sv.local AS src, tv.local AS dst, e.src_dense, e.dst_dense\n` +
          `  FROM read_parquet(${list(edgeUrls)}) e\n` +
          `  JOIN held a ON a.dense_id = e.src_dense\n` +
          `  JOIN held b ON b.dense_id = e.dst_dense\n` +
          `  LEFT JOIN vis sv ON sv.dense_id = e.src_dense\n` +
          `  LEFT JOIN vis tv ON tv.dense_id = e.dst_dense\n` +
          `  WHERE (${ranges('e.src_dense')})\n` +
          `    AND (sv.dense_id IS NOT NULL OR tv.dense_id IS NOT NULL)${long}\n` +
          `), anchor AS (\n` +
          `  SELECT h.dense_id, h.x, h.y,\n` +
          `         ((SELECT count(*) FROM vis) + row_number() OVER (ORDER BY h.dense_id) - 1)::INTEGER AS local\n` +
          `  FROM held h WHERE h.dense_id IN (\n` +
          `    SELECT src_dense FROM span WHERE src IS NULL\n` +
          `    UNION SELECT dst_dense FROM span WHERE dst IS NULL)\n)`;

    const pointsSql =
      edgeUrls.length === 0
        ? `${withEdges}\nSELECT local, dense_id, x, y, cat, matched, TRUE AS mark FROM vis ORDER BY local`
        : `${withEdges}\nSELECT local, dense_id, x, y, cat, matched, TRUE AS mark FROM vis\n` +
          `UNION ALL\nSELECT local, dense_id, x, y, 0, NULL::BIGINT, FALSE AS mark FROM anchor\n` +
          `ORDER BY local`;

    const points = await query(pointsSql);
    const links =
      edgeUrls.length === 0
        ? []
        : await query(
            read.lines.positioned
              ? `${withEdges}\nSELECT coalesce(sv.local, sa.local) AS src, coalesce(dv.local, da.local) AS dst\n` +
                `FROM links l\n` +
                `LEFT JOIN vis sv ON sv.dense_id = l.src_dense\n` +
                `LEFT JOIN vis dv ON dv.dense_id = l.dst_dense\n` +
                `LEFT JOIN anchor sa ON sa.dense_id = l.src_dense\n` +
                `LEFT JOIN anchor da ON da.dense_id = l.dst_dense`
              : `${withEdges}\nSELECT coalesce(sp.src, sa.local) AS src, coalesce(sp.dst, da.local) AS dst\n` +
                `FROM span sp\n` +
                `LEFT JOIN anchor sa ON sa.dense_id = sp.src_dense\n` +
                `LEFT JOIN anchor da ON da.dense_id = sp.dst_dense`,
          );

    // What the lines cost, on the ledger rather than beside it. Under a level read this is the
    // relation's own level set; under a payload read it is the adjacency tiles the plan named.
    //
    // Bounded by the SAME intervals the edge query is — `ranges('src_dense')` — because both
    // sides of a relation's file are addressed by the source's `dense_id` and neither container
    // gives an adjacency tile a row-group ordinal of its own: what locates it is the footer's
    // box on `src_dense`, which is what this weighs against.
    const edgeBytes = await bytesOf(
      edgeUrls,
      read.lines.columns,
      { column: 'src_dense', runs, span },
    );
    const rows = points.length;
    const denseIds = new BigUint64Array(rows);
    const positions = new Float32Array(rows * 2);
    const categories = new Uint32Array(rows);
    let marks = 0;
    let matched = 0;
    for (let i = 0; i < rows; i += 1) {
      const row = points[i]!;
      denseIds[i] = idOf(row['dense_id'], `${address.type}.dense_id`);
      positions[i * 2] = floatOf(row['x'] ?? 0, `${address.type}.x`);
      positions[i * 2 + 1] = floatOf(row['y'] ?? 0, `${address.type}.y`);
      categories[i] = Number(row['cat'] ?? 0) >>> 0;
      // `mark` is TRUE down the sample and FALSE down the anchors, and the answer is ordered by
      // `local`, so this is a prefix length rather than a count.
      if (row['mark'] === true || row['mark'] === 1) marks = i + 1;
      if (i === 0) matched = Number(row['matched'] ?? 0);
    }
    const edges = new Uint32Array(links.length * 2);
    for (let i = 0; i < links.length; i += 1) {
      edges[i * 2] = Number(links[i]!['src']) >>> 0;
      edges[i * 2 + 1] = Number(links[i]!['dst']) >>> 0;
    }

    return {
      type: address.type,
      box,
      level,
      stride,
      matched,
      matchedAt: read.matchedAt,
      denseIds,
      positions,
      categories,
      marks,
      links: edges,
      undrawn: read.lines.undrawn,
      cost: {
        requests: runs.length + pinRuns.length,
        bytes: bytes + edgeBytes,
        ms: Date.now() - started,
        tiles: held.length + read.pinTiles.length,
        ofTiles: read.all.length,
      },
    };
  };
}
