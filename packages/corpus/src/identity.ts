/**
 * A vertex by its identity: the index seek, the key ranges its footers publish, and the byte order
 * those ranges compare in — with the scan of the identity column where a type publishes no index.
 */

import type { CorpusAddressing, VertexAddress } from './address.js';
import type { QueryFn, QueryRow } from './query.js';
import { ascending, CorpusReadError, distinct, floatOf, ident, idOf, list, lit } from './sql.js';
import {
  PAYLOAD_ADDRESS,
  PAYLOAD_COORDINATES,
  PAYLOAD_IDENTITY,
} from './vocabulary.generated.js';

/**
 * One vertex, **placed** — an address, a position, and every column the row carries.
 *
 * It carries an `x`, a `y` and a `dense_id` because a camera reads tiles and a canvas cannot draw
 * an identity.
 */
export interface PlacedVertex {
  readonly type: string;
  /** The subject IRI — the identity — or `null` when this payload does not carry one. */
  readonly id: string | null;
  /** The address. It is a `BigInt`, and it does not survive a re-layout. */
  readonly denseId: bigint;
  readonly x: number;
  readonly y: number;
  /** Every column that is not `dense_id`, `subject`, `x` or `y`. */
  readonly fields: QueryRow;
}

/** What {@link Corpus.node} takes beyond the identity. */
export interface NodeParams {
  /** Narrow the search to one vertex type. Without it every type carrying an identity is searched. */
  type?: string;
}

/** The four columns an answer reads by name; everything else is payload. */
/**
 * The payload columns {@link PlacedVertex} already surfaces as NAMED members, so
 * that `fields` carries each column exactly once.
 *
 * Read by ROLE and not written down: the address, the identity and the two
 * coordinates are the four this struct has a field for. `cluster_id` is
 * deliberately absent — nothing here surfaces it, so it belongs in `fields` —
 * and that is why this set is four where `fossil-graph`'s is five. The two ask
 * different questions: that one is *what is not user data*, this one is *what
 * this type already answers another way*. They were two literals and nothing
 * said so; `corpus.bnf` is where the table lives now.
 */
const RESERVED = new Set<string>([
  ...PAYLOAD_ADDRESS,
  ...PAYLOAD_IDENTITY,
  ...PAYLOAD_COORDINATES,
]);

/** The column the identity is read from. See {@link Corpus.node} for why it is not `dense_id`. */
export const IDENTITY = PAYLOAD_IDENTITY[0]!;

/**
 * A key as the bytes a Parquet statistic is compared in.
 *
 * **`<` on two JavaScript strings is the wrong comparison here, and silently so.** Parquet orders a
 * string column by unsigned UTF-8 bytes, and so does DuckDB's default collation, so that is the
 * order an index tile is sorted in and the order its footer's min/max bound. JavaScript compares
 * UTF-16 code units, which agrees with UTF-8 order for everything below U+E000 and disagrees above
 * it: a surrogate pair's code units sort below the private-use area and its bytes sort above it. An
 * IRI that reached there would be pruned out of the one tile holding it and reported as absent,
 * which is the failure a lookup cannot notice. So the comparison is on bytes.
 */
const UTF8 = new TextEncoder();
function utf8(value: string): Uint8Array {
  return UTF8.encode(value);
}

/** Unsigned byte order, which is Parquet's order for a string column. */
function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i += 1) {
    if (a[i] !== b[i]) return a[i]! - b[i]!;
  }
  return a.length - b.length;
}

/**
 * Whether a tile bounded by `[lo, hi]` can hold `key` — and it answers `true` when it cannot tell.
 *
 * The upper clause is not just `key <= hi`, because a Parquet writer may TRUNCATE a long string
 * statistic rather than store it whole. parquet-rs truncates a max upward — it increments the last
 * byte, so the stored bound is still at least every value in the tile — and a min downward, which
 * makes the plain comparison safe against the writer this corpus has. A writer that truncated a max
 * WITHOUT incrementing would store a prefix of the real one, and every key extending that prefix
 * would be excluded from the only tile that holds it. The prefix test costs one comparison and
 * removes the whole class.
 */
function couldHold(key: Uint8Array, lo: Uint8Array, hi: Uint8Array): boolean {
  if (compareBytes(key, lo) < 0) return false;
  if (compareBytes(key, hi) <= 0) return true;
  return key.length > hi.length && compareBytes(key.subarray(0, hi.length), hi) === 0;
}

export const vertexOf = (type: string, row: QueryRow): PlacedVertex => {
  const fields: QueryRow = {};
  for (const [key, value] of Object.entries(row)) {
    if (!RESERVED.has(key)) fields[key] = value;
  }
  return {
    type,
    id: typeof row[IDENTITY] === 'string' ? (row[IDENTITY] as string) : null,
    denseId: idOf(row['dense_id'], `${type}.dense_id`),
    x: floatOf(row['x'] ?? 0, `${type}.x`),
    y: floatOf(row['y'] ?? 0, `${type}.y`),
    fields,
  };
};

/**
 * Where each INDEX tile's keys begin and end, read from its own footer **once per type.**
 *
 * The same move {@link tileBoxes} makes for `x`/`y`, for the one column an index is sorted by,
 * and for the same reason: the arithmetic says which tiles exist and only the footers say which
 * ones a value can be in. The difference is which side does the pruning. A window leaves it to
 * the engine because a box over `x`/`y` is a conjunctive range and DuckDB prunes one; a lookup
 * cannot, because the predicate is a DISJUNCTION and DuckDB prunes none of those over a VARCHAR
 * column — measured on v1.5.3 over a million-vertex corpus, `key = 'x'` prunes to one tile and
 * reads 3.98 MB while `key IN ('x','y')`, or the same spelled with `OR`, prunes to none and reads
 * all 245 index tiles, 44.1 MB. So the pruning is done here, from the same statistics the engine
 * declined to use, and the batch stays one query.
 *
 * **It cannot work under `rowgroups` and does not pretend to.** There the whole index is one file
 * and a row group has no URL, so there is nothing to address and nothing to leave out of the
 * list: `null` here means the batched query over the whole index is what there is. Same for a
 * one-file index, where pruning has nothing to remove.
 *
 * A tile whose footer carries no statistics for the key column keeps `lo`/`hi` at `null` and is
 * always read — the conservative answer, and the same call {@link tileBoxes} makes for a tile
 * with no box.
 */
interface KeyRange {
  readonly url: string;
  /** The tile's key bounds as bytes, or `null` when its footer declared none. */
  readonly lo: Uint8Array | null;
  readonly hi: Uint8Array | null;
}

export interface Identity {
  findByIdentity(ids: readonly string[], only?: string): Promise<PlacedVertex[]>;
  node(id: string, params?: NodeParams): Promise<PlacedVertex | null>;
}

export function identityOf(reads: {
  readonly query: QueryFn;
  readonly addressing: CorpusAddressing;
  readonly payloadFiles: ReadonlyMap<string, readonly string[]>;
  readonly has: (type: string, column: string) => boolean;
}): Identity {
  const { query, addressing, payloadFiles, has } = reads;

  const keyRanges = new Map<string, Promise<readonly KeyRange[] | null>>();

  const indexRanges = (type: VertexAddress): Promise<readonly KeyRange[] | null> => {
    const cached = keyRanges.get(type.type);
    if (cached) return cached;
    const index = type.index!;
    const loading = (async (): Promise<readonly KeyRange[] | null> => {
      if (index.container === 'rowgroups') return null;
      const urls = distinct([...index.files()]);
      if (urls.length < 2) return null;
      // `coalesce(stats_min_value, stats_min)` for the reason `tileBoxes` gives: Parquet's original
      // statistics fields are defined as a signed comparison and a writer that gets that right
      // leaves them empty, so a reader that knows only the deprecated pair concludes the footer
      // carries no bound at all. Read either.
      const rows = await query(
        `SELECT file_name AS file, ` +
          `min(coalesce(stats_min_value, stats_min)) AS lo, ` +
          `max(coalesce(stats_max_value, stats_max)) AS hi ` +
          `FROM parquet_metadata(${list(urls)}) ` +
          `WHERE path_in_schema = ${lit(index.orderedBy)} GROUP BY 1`,
      );
      const byUrl = new Map<string, KeyRange>();
      for (const row of rows) {
        const url = String(row['file']);
        const lo = row['lo'];
        const hi = row['hi'];
        byUrl.set(
          url,
          typeof lo === 'string' && typeof hi === 'string'
            ? { url, lo: utf8(lo), hi: utf8(hi) }
            : { url, lo: null, hi: null },
        );
      }
      // A tile whose name did not come back verbatim is one this cannot exclude either.
      return urls.map((url) => byUrl.get(url) ?? { url, lo: null, hi: null });
    })();
    keyRanges.set(type.type, loading);
    return loading;
  };

  /**
   * The index tiles a batch of keys can be in. Linear in tiles x keys, and deliberately: the tiles
   * of an index ARE disjoint and sorted, which would make this a binary search per key, but that is
   * a property of the writer rather than of the format and a reader that assumed it would answer a
   * badly written corpus with silence. A few hundred tiles against a caller's own batch is nothing
   * beside the read it decides.
   */
  const indexFilesFor = (ranges: readonly KeyRange[], keys: readonly Uint8Array[]): string[] =>
    ranges
      .filter((r) => r.lo === null || r.hi === null || keys.some((k) => couldHold(k, r.lo!, r.hi!)))
      .map((r) => r.url);

  /**
   * Every vertex whose identity is in `ids` — **one scan for the whole batch, per type.**
   *
   * The scan is the expense the whole surface is most able to multiply (see {@link Corpus.node}),
   * and a walk seeded by ten identities that ran ten of them would pay ten times for one answer.
   * Nothing bounds the `IN` list: it is the caller's own set, and the corpus cannot refuse it.
   */
  const findByIdentity = async (
    ids: readonly string[],
    only?: string,
  ): Promise<PlacedVertex[]> => {
    if (ids.length === 0) return [];
    const candidates = (only === undefined ? addressing.types : [addressing.vertexType(only)]).filter((type) =>
      has(type.type, IDENTITY),
    );
    if (candidates.length === 0) {
      throw new CorpusReadError(
        `no vertex type here carries a ${IDENTITY} column, so nothing in this corpus has a name ` +
          `that survives a re-layout — where the identity lives when the drawing tile does not ` +
          `carry it is an open convention`,
      );
    }
    const found: PlacedVertex[] = [];
    for (const type of candidates) {
      const index = type.index;
      if (index === null) {
        // **The scan**, and it is the whole cost this member has. There is no index from a
        // subject to an address, and the payload's own footers do not help: the rows are in
        // Morton order and subjects are not, so every tile's `min`/`max` for `subject` overlaps
        // every other's and the engine skips nothing. At five million vertices the `subject`
        // column is 8.016 compressed bytes per row, so one lookup reads about 40 MB — column
        // pruning is the only thing keeping it off the other six.
        const rows = await query(
          `SELECT * FROM read_parquet(${list(payloadFiles.get(type.type)!)}) ` +
            `WHERE ${ident(IDENTITY)} IN (${ids.map(lit).join(', ')})`,
        );
        for (const row of rows) found.push(vertexOf(type.type, row));
        continue;
      }

      // **The seek**, and it is addressed rather than searched at both ends.
      //
      // First the index: two columns over tiles sorted by the key with disjoint ranges, which is
      // the arrangement that lets a reader go to the one tile a value can be in. The payload cannot
      // be arranged that way and keep the Morton order a window depends on, which is why the index
      // is a second table rather than a second sort.
      //
      // **The arrangement is right and DuckDB does not exploit it**, so the list of files is what
      // exploits it. `key = 'x'` prunes to one index tile and reads 3.98 MB; `key IN ('x','y')`, or
      // the same spelled with `OR`, prunes to NONE and reads all 245 of them, 44.1 MB — measured on
      // v1.5.3 over a million-vertex corpus, and DuckDB prunes no disjunction over a VARCHAR column
      // at all. Taking the batch apart to get one equality per query is NOT the fix and was tried:
      // it crosses back over the batched read at about eleven seeds (11 x 3.98 > 44.1), and it is
      // a lookup per seed, which is the one cost this surface is most able to multiply.
      //
      // So {@link indexRanges} reads the same statistics the engine declined to use, once per
      // corpus, and the batch stays ONE query over the handful of files its keys can be in. Under
      // `rowgroups` there is nothing to leave out — one file, and a row group has no URL — and the
      // batched read over the whole index is what there is.
      const ranges = await indexRanges(type);
      const keys = ids.map(utf8);
      const indexFiles =
        ranges === null ? distinct([...index.files()]) : indexFilesFor(ranges, keys);
      // No tile's range can hold any of these keys, which is an answer and not a failure to look.
      if (indexFiles.length === 0) continue;
      const hits = await query(
        `SELECT ${ident(index.orderedBy)} AS id, dense_id ` +
          `FROM read_parquet(${list(indexFiles)}) ` +
          `WHERE ${ident(index.orderedBy)} IN (${ids.map(lit).join(', ')})`,
      );
      if (hits.length === 0) continue;

      // Then the payload, at the tiles those addresses NAME — not all of them. This is the half
      // that turns a lookup into arithmetic: `tileOf` is a shift.
      const addresses = hits.map((row) => idOf(row.dense_id, `${type.type}.dense_id`));
      const tiles = [...new Set(type.tilesOf(addresses))].sort(ascending);
      const rows = await query(
        `SELECT * FROM read_parquet(${list(distinct(tiles.map((k) => type.tileUrl(k))))}) ` +
          `WHERE dense_id IN (${addresses.join(', ')})`,
      );
      // An index that names an address the payload does not have is a corpus defect, not a miss:
      // `packages/corpus/guards`' `index-agrees-with-the-payload` is what catches it, and a reader that
      // quietly returned fewer rows than the index promised would hide exactly that.
      if (rows.length !== addresses.length) {
        throw new CorpusReadError(
          `${type.type}'s index names ${addresses.length} address(es) and the payload has ` +
            `${rows.length} of them — the index disagrees with the tiles it indexes`,
        );
      }
      for (const row of rows) found.push(vertexOf(type.type, row));
    }
    return found;
  };

  return {
    findByIdentity,

    /**
     * One vertex by identity.
     *
     * **`id` is the subject IRI, never the `dense_id`** — a re-layout renumbers every vertex, so
     * an address held outside the corpus names a different one after the next write. That decision,
     * the alternative it refuses, what would reverse it, and why `is_primary` is not consulted are
     * on `/docs/design/corpus` under *A reader is handed the subject IRI*.
     *
     * **What it costs, measured rather than asserted.** Where the type declares an `index:` this is
     * a seek: two reads, no scan of either table, and see {@link Corpus.types} — `indexed` says
     * which of the two a type gets. The index read is one query for the whole batch over only the
     * index tiles whose own footers say a key could be in them, because this engine prunes no
     * disjunction over a string column and {@link indexRanges} is where that is made up for. Where
     * the type declares no index, it is a scan of the `subject` column over
     * every tile, because the rows are in Morton order and subjects are not, so every tile's
     * `min`/`max` overlaps every other's and the footers prune nothing. At five million vertices
     * that column is 8.016 compressed bytes per row, so one lookup reads about 40 MB. Both answers
     * are the same row; only one is cheap.
     *
     * @throws {CorpusReadError} when no vertex type carries an identity column at all, or when two
     *   types claim the same IRI.
     */
    async node(id, params = {}) {
      if (typeof id !== 'string') {
        throw new TypeError(
          `node takes a subject IRI; got ${typeof id}. A dense_id is an address, and a re-layout ` +
            `gives it to a different vertex.`,
        );
      }
      const found = await findByIdentity([id], params.type);
      if (found.length > 1) {
        throw new CorpusReadError(
          `${id} names ${found.length} vertices, in ${[...new Set(found.map((v) => v.type))].join(', ')}; ` +
            `an identity is unique within its type and this corpus reuses one across types`,
        );
      }
      return found[0] ?? null;
    },
  };
}
