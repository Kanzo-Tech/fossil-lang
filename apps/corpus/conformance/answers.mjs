/**
 * The four questions a corpus answers, answered a second time.
 *
 * `reader.mjs` is the second implementation of the ADDRESSING — which URL a tile has. This is the
 * second implementation of the ANSWERS: what is inside, what is in a rectangle, what one vertex is,
 * and what is within N hops. `openCorpus` in `@fossil-lang/corpus` is the first, published and typed;
 * this is plain Node over the `duckdb` binary, and it shares no line of answer logic with it.
 *
 * # Why a second one at all, when `packages/corpus/tests/corpus.test.ts` already cross-checks
 *
 * That file runs the published API against this corpus and compares every answer to SQL it writes
 * itself, which is strictly better than a constant — but it is **one implementation checked against
 * its own author's second thought**. Two independent readers disagreeing is a different signal, and
 * it is the one the addressing half already has.
 *
 * # Where the expected numbers come from, and it is neither of us
 *
 * `expected.json`'s `answers` block is derived from a **full scan** — every tile of every type read
 * with no addressing at all, filtered in SQL. That is a third thing, and it is the point: both
 * implementations PRUNE, both must land on what an unpruned read sees. A wrong shift, a doubled
 * orientation or a hard-coded 4,096 moves one of them off a number that was never computed by
 * either.
 *
 * # What this cannot prove
 *
 * - **That the expected numbers are the right questions to ask.** They are sizes and a few
 *   identifying values, not whole result sets; a reader that returned the right count of the wrong
 *   rows passes the count and fails the spot checks, and a reader wrong in a way neither covers
 *   passes both.
 * - **That either implementation prunes.** Both answers would be identical from a full scan. What
 *   the addressing half proves is that the URLs are right; that they are the URLs actually fetched
 *   is not observable from a result.
 * - **That either container is faster.** Both are read here through the same `read_parquet`, and
 *   the whole argument for the row-group one is a count of HTTP requests a local file never makes.
 *   What `containers.mjs` asks is the other half: that the answers are the same either way.
 */

import { query, lit } from "../guards/duck.mjs";
import { resolve } from "./reader.mjs";

/** A DuckDB list literal of paths — how one query spans a set of tiles. */
const list = (paths) => `[${paths.map((p) => `'${lit(p)}'`).join(", ")}]`;

/** The four columns an answer reads by name. Everything else is payload. */
const RESERVED = new Set(["dense_id", "subject", "x", "y"]);

/** Where an identity is read from. The guards' own convention, not a manifest flag. */
const IDENTITY = "subject";

const num = (v) => (v === null || v === undefined ? null : Number(v));

/**
 * What is inside.
 *
 * The counts come from the MANIFEST rather than from `count(*)`, because that is what the manifest
 * declaring them is for — a corpus that declares a count its bytes do not have is what
 * `guards/check.mjs`'s `declared-count` exists to catch, and repeating the check here would move
 * that question into the wrong file.
 */
export function types(root, base = "") {
  const corpus = resolve(root, base);
  const vertices = corpus.types.map((t) => {
    const columns = query(
      `DESCRIBE SELECT * FROM read_parquet(${list([`${root}/${t.tileUrl(0)}`])})`,
    );
    const fields = columns.map((c) => String(c.column_name));
    return {
      type: t.type,
      chunk_size: t.chunkSize,
      fields,
      identity: fields.includes(IDENTITY) ? IDENTITY : null,
      geometry: fields.includes("x") && fields.includes("y"),
      payload: fields.filter((f) => !RESERVED.has(f)),
    };
  });
  const edges = corpus.edges.map((e) => ({
    edge_type: e.edgeType,
    src_type: e.srcType,
    dst_type: e.dstType,
    directions: e.directions,
  }));
  return { vertices, edges };
}

/**
 * Every payload FILE of one type, resolved against the corpus root.
 *
 * Distinct, which is what makes this one function for both containers: file-per-tile gives one URL
 * per tile and the row-group container gives the same URL for all of them, and a list that repeated
 * it would read the relation once per tile and count every row that many times.
 */
function allTiles(root, vertex, count) {
  const tiles = Math.ceil(count / vertex.chunkSize);
  return [...new Set(Array.from({ length: tiles }, (_, k) => `${root}/${vertex.tileUrl(k)}`))];
}

/** Every index file of one type, resolved against the corpus root. Distinct, for the same reason. */
function indexTiles(root, vertex) {
  return [
    ...new Set(
      Array.from({ length: vertex.index.tiles }, (_, k) => `${root}/${vertex.index.tileUrl(k)}`),
    ),
  ];
}

/** `x >= ? AND x < ? AND …` — a half-open box, so two adjacent windows share no vertex. */
const boxOf = ({ x, y, w, h }) =>
  `x >= ${x} AND x < ${x + w} AND y >= ${y} AND y < ${y + h}`;

/**
 * The vertices in a rectangle and the edges incident to them.
 *
 * Incident, not internal: an edge whose far endpoint is outside the box is still an edge of a
 * vertex that was drawn, and leaving it out is how a window under-reports degree. Which
 * orientations are read decides which half of "incident" is answered, and `gaps` names the other.
 */
export function window(root, count, { x, y, w, h, type, directions = ["src", "dst"] }) {
  const corpus = resolve(root, "");
  const vertex = corpus.vertexType(type);
  const urls = allTiles(root, vertex, count);
  const box = boxOf({ x, y, w, h });

  const rows = query(
    `SELECT * FROM read_parquet(${list(urls)}) WHERE ${box} ORDER BY dense_id`,
  );
  const tiles = [...new Set(rows.map((r) => Math.floor(Number(r.dense_id) / vertex.chunkSize)))].sort(
    (a, b) => a - b,
  );

  const plan = corpus.window({ type: vertex.type, tiles, directions });
  const reads = corpus.edgeReads({ type: vertex.type, tiles, directions });
  const seen = new Set();
  let edges = 0;
  if (tiles.length > 0) {
    for (const read of reads) {
      // Filtered by the adjacency's OWN column. `by_source/tile{k}` holds the edges whose
      // `src_dense` is in tile k, so an `OR dst_dense IN (…)` here answers a question nobody asked
      // — it drags in edges whose source is outside the window. That was written, and it read 153
      // where the full scan says 152.
      const found = query(
        `SELECT src_dense, dst_dense FROM read_parquet(${list([`${root}/${read.url}`])}) ` +
          `WHERE ${read.column} IN (SELECT dense_id FROM read_parquet(${list(urls)}) WHERE ${box})`,
      );
      // One relation stored twice: an edge found by both orientations is ONE edge, and
      // concatenating is how a self-type window doubles its own count.
      for (const row of found) seen.add(`${row.src_dense} ${row.dst_dense}`);
    }
    edges = seen.size;
  }

  return {
    type: vertex.type,
    vertices: rows.length,
    tiles,
    edges,
    complete: plan.complete,
    gaps: plan.gaps.map((g) => g.reason).sort(),
  };
}

/**
 * One vertex by identity, which is the subject IRI.
 *
 * A scan of the `subject` column over every tile of the type, and it cannot be anything else here:
 * the rows are in Morton order and subjects are not, so no footer statistic prunes and the corpus
 * publishes no index from one to the other. That cost is the answer's, not this file's.
 */
export function node(root, count, id, type) {
  const corpus = resolve(root, "");
  const vertex = corpus.vertexType(type);
  const rows =
    vertex.index === null
      ? // The scan. Every tile of the type, because the rows are in Morton order
        // and subjects are not, so no footer prunes.
        query(
          `SELECT * FROM read_parquet(${list(allTiles(root, vertex, count))}) ` +
            `WHERE ${IDENTITY} = '${lit(id)}'`,
        )
      : // The seek, in two reads: the index tiles — sorted by the key with
        // disjoint ranges, so the engine prunes to one — and then the ONE
        // payload tile the address it returns names.
        (() => {
          const hits = query(
            `SELECT dense_id FROM read_parquet(${list(indexTiles(root, vertex))}) ` +
              `WHERE ${vertex.index.orderedBy} = '${lit(id)}'`,
          );
          if (hits.length === 0) return [];
          const dense = Number(hits[0].dense_id);
          const tile = Math.floor(dense / vertex.chunkSize);
          const found = query(
            `SELECT * FROM read_parquet(${list([`${root}/${vertex.tileUrl(tile)}`])}) ` +
              `WHERE dense_id = ${dense}`,
          );
          if (found.length === 0) {
            throw new Error(
              `${vertex.type}'s index names dense_id ${dense} and tile ${tile} does not have it`,
            );
          }
          return found;
        })();
  if (rows.length === 0) return null;
  if (rows.length > 1) throw new Error(`${id} names ${rows.length} vertices of ${vertex.type}`);
  const row = rows[0];
  return {
    type: vertex.type,
    id: String(row[IDENTITY]),
    dense_id: Number(row.dense_id),
    x: num(row.x),
    y: num(row.y),
  };
}

/**
 * Everything within `depth` hops of a set of identities.
 *
 * The walk is over `dense_id` because that is what an adjacency tile holds — both endpoints are
 * addresses and neither is an identity — and the seeds are resolved to addresses first. `frontier`
 * is the vertices the last hop reached and whose own edges were never opened: without it the answer
 * looks whole, because the edges BETWEEN two frontier vertices are the ones missing.
 */
export function neighbours(root, count, ids, { depth = 1, type, directions = ["src", "dst"] } = {}) {
  const corpus = resolve(root, "");
  const vertex = corpus.vertexType(type);
  const urls = allTiles(root, vertex, count);

  const seeds = [];
  const missing = [];
  for (const id of ids) {
    const found = node(root, count, id, type);
    if (found === null) missing.push(id);
    else seeds.push(found.dense_id);
  }

  const reached = new Set(seeds);
  let frontier = new Set(seeds);
  const edges = new Set();

  for (let hop = 0; hop < depth && frontier.size > 0; hop += 1) {
    const tiles = [...new Set([...frontier].map((d) => Math.floor(d / vertex.chunkSize)))].sort(
      (a, b) => a - b,
    );
    const reads = corpus.edgeReads({ type: vertex.type, tiles, directions });
    const next = new Set();
    const inList = [...frontier].join(", ");
    for (const read of reads) {
      const found = query(
        `SELECT src_dense, dst_dense FROM read_parquet(${list([`${root}/${read.url}`])}) ` +
          `WHERE ${read.column} IN (${inList})`,
      );
      for (const row of found) {
        const src = Number(row.src_dense);
        const dst = Number(row.dst_dense);
        edges.add(`${src} ${dst}`);
        for (const end of [src, dst]) {
          if (!reached.has(end)) next.add(end);
        }
      }
    }
    for (const d of next) reached.add(d);
    frontier = next;
  }

  const rows =
    reached.size === 0
      ? []
      : query(
          `SELECT count(*) AS n FROM read_parquet(${list(urls)}) ` +
            `WHERE dense_id IN (${[...reached].join(", ")})`,
        );

  return {
    type: vertex.type,
    depth,
    seeds: seeds.length,
    missing: missing.length,
    vertices: rows.length === 0 ? 0 : Number(rows[0].n),
    edges: edges.size,
    frontier: frontier.size,
    // The walk exhausted the component only when the last hop reached nothing new.
    complete: frontier.size === 0 && missing.length === 0,
  };
}
