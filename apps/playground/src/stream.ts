/**
 * Streaming a corpus: which tiles a question opens, and what that costs in bytes.
 *
 * ## The claim
 *
 * > A reader computes every URL it wants before it emits the first request.
 *
 * `/docs/format/conventions/addressing` states it and measures it; this module is that
 * sentence with a network tab attached. Nothing here is a simulation — the requests are real
 * `fetch` calls against this app's own origin, and the byte counts are the responses.
 *
 * ## The three steps, and which of them costs anything
 *
 * 1. **Address.** `resolveCorpus` turns the manifests into tile URLs, synchronously, with no
 *    engine and no WASM. Costs the three small YAML manifests, and nothing else.
 * 2. **Read the footer, once.** Which tiles a *rectangle* touches is not arithmetic — it comes
 *    from the per-row-group `x`/`y` statistics in the Parquet footer, and reading a footer
 *    needs a Parquet reader. `@fossil-lang/graph` deliberately does not carry one; the host
 *    does. Here the host is DuckDB-WASM, and {@link FOOTER_SQL} is the one query. The footer
 *    is the one thing a reader holds that grows with N, and it is bought once per corpus.
 * 3. **Fetch the runs.** Under `container: rowgroups` the whole payload is ONE
 *    `tiles.parquet`, and row group `k` IS tile `k`. Row groups are laid out consecutively
 *    and contiguously, so a run of adjacent tiles is a single byte interval — one HTTP
 *    `Range` request. That is why a rectangle costing O(√n) runs costs O(√n) requests.
 *
 * ## Why the fetching is done here and not by the engine
 *
 * DuckDB-WASM would happily read the remote Parquet itself and do its own range requests. It
 * runs in a Worker, and a Worker's resource timings do not appear in this thread — the bytes
 * would be real and unobservable, which for a panel whose entire job is to show the bytes is
 * the wrong trade. So the engine is used for the footer, whose answer is small and structural,
 * and the payload is fetched by this module where every response can be weighed.
 *
 * Verified against the generated corpus by `scripts/verify-stream.mjs`, which runs this
 * arithmetic in Node over a real HTTP server and checks that the runs are contiguous, that
 * the selected tiles are the ones holding the matching vertices, and that no byte is fetched
 * twice.
 */

/** One tile, as the footer describes it: where its bytes are, and the box its geometry fills. */
export interface TileBox {
  /** Tile ordinal — under `rowgroups`, the row-group index, which IS the tile. */
  tile: number;
  /** Rows in the tile. `chunk_size` for every tile but the last. */
  rows: number;
  /** Byte offset of the tile's first page within the file. */
  start: number;
  /** Compressed bytes across every column of the tile. */
  bytes: number;
  xlo: number;
  xhi: number;
  ylo: number;
  yhi: number;
}

/** A rectangle over the layout's coordinates — the question. */
export interface Rect {
  xlo: number;
  xhi: number;
  ylo: number;
  yhi: number;
}

/** A maximal set of adjacent tiles, and the single byte interval that holds them. */
export interface Run {
  /** First tile ordinal in the run, inclusive. */
  first: number;
  /** Last tile ordinal in the run, inclusive. */
  last: number;
  /** Byte offset of the run's first byte. */
  start: number;
  /** Length of the run in bytes — what one `Range` request asks for. */
  bytes: number;
}

/**
 * The footer query: one row per tile, carrying its byte interval and its geometric box.
 *
 * `min(coalesce(dictionary_page_offset, data_page_offset))` is where a row group's bytes
 * begin — the dictionary page comes first when there is one, and `file_offset` is not
 * reliably populated by every writer, so the page offsets are the honest source.
 * `sum(total_compressed_size)` over its columns is how far it runs.
 *
 * `stats_min_value` / `stats_max_value` are `VARCHAR` in `parquet_metadata`'s schema
 * regardless of the column's type, so they are cast rather than trusted.
 */
export function FOOTER_SQL(file: string): string {
  const f = file.replace(/'/g, "''");
  return `
    SELECT
      row_group_id AS tile,
      any_value(row_group_num_rows) AS rows,
      min(coalesce(dictionary_page_offset, data_page_offset)) AS start,
      sum(total_compressed_size) AS bytes,
      min(CASE WHEN path_in_schema = 'x' THEN CAST(stats_min_value AS DOUBLE) END) AS xlo,
      max(CASE WHEN path_in_schema = 'x' THEN CAST(stats_max_value AS DOUBLE) END) AS xhi,
      min(CASE WHEN path_in_schema = 'y' THEN CAST(stats_min_value AS DOUBLE) END) AS ylo,
      max(CASE WHEN path_in_schema = 'y' THEN CAST(stats_max_value AS DOUBLE) END) AS yhi
    FROM parquet_metadata('${f}')
    GROUP BY row_group_id
    ORDER BY row_group_id
  `;
}

/** Coerce one `parquet_metadata` row. Widths arrive as `bigint` from some hosts and
 *  `number` from others — the graph package's `QueryFn` contract says so explicitly. */
export function toTileBox(row: Record<string, unknown>): TileBox {
  const num = (v: unknown): number => (typeof v === 'bigint' ? Number(v) : Number(v));
  return {
    tile: num(row.tile),
    rows: num(row.rows),
    start: num(row.start),
    bytes: num(row.bytes),
    xlo: num(row.xlo),
    xhi: num(row.xhi),
    ylo: num(row.ylo),
    yhi: num(row.yhi),
  };
}

/** The bounding box of every tile — the extent of the whole graph, from the footer alone. */
export function extentOf(boxes: readonly TileBox[]): Rect | null {
  if (boxes.length === 0) return null;
  return boxes.reduce<Rect>(
    (acc, b) => ({
      xlo: Math.min(acc.xlo, b.xlo),
      xhi: Math.max(acc.xhi, b.xhi),
      ylo: Math.min(acc.ylo, b.ylo),
      yhi: Math.max(acc.yhi, b.yhi),
    }),
    { xlo: boxes[0]!.xlo, xhi: boxes[0]!.xhi, ylo: boxes[0]!.ylo, yhi: boxes[0]!.yhi },
  );
}

/** A rectangle covering `fraction` of each axis of `extent`, centred on (`cx`, `cy`) given
 *  in normalised [0,1] coordinates. The panel's "how big a question" control. */
export function windowIn(extent: Rect, fraction: number, cx = 0.5, cy = 0.5): Rect {
  const w = (extent.xhi - extent.xlo) * fraction;
  const h = (extent.yhi - extent.ylo) * fraction;
  const x = extent.xlo + (extent.xhi - extent.xlo) * cx;
  const y = extent.ylo + (extent.yhi - extent.ylo) * cy;
  return { xlo: x - w / 2, xhi: x + w / 2, ylo: y - h / 2, yhi: y + h / 2 };
}

/**
 * The tiles a rectangle touches — every tile whose box intersects it.
 *
 * This is tile granularity, and it over-reads on purpose: a tile is the unit that has an
 * address, so a tile one of whose vertices is inside the window is fetched whole. The
 * addressing page measures the waste across nine windows at 1.10×–1.20×, mean 1.14×.
 */
export function selectTiles(boxes: readonly TileBox[], rect: Rect): TileBox[] {
  return boxes.filter((b) => b.xlo <= rect.xhi && b.xhi >= rect.xlo && b.ylo <= rect.yhi && b.yhi >= rect.ylo);
}

/**
 * Collapse selected tiles into maximal runs of adjacent tiles, each a single byte interval.
 *
 * Two tiles join a run when they are consecutive ordinals AND their bytes actually abut. The
 * second condition is not paranoia about the format — it is what makes the returned interval
 * honest. Parquet does not promise that row groups are written back to back, and a run whose
 * members are not contiguous would name a range containing bytes belonging to nobody. Against
 * the corpus this app generates, every boundary abuts (measured: 0 gaps in 244), so the
 * O(√n)-runs claim holds as O(√n) requests — but it is checked rather than assumed.
 */
export function runsOf(tiles: readonly TileBox[]): Run[] {
  const sorted = [...tiles].sort((a, b) => a.tile - b.tile);
  const runs: Run[] = [];
  for (const t of sorted) {
    const open = runs[runs.length - 1];
    if (open && t.tile === open.last + 1 && open.start + open.bytes === t.start) {
      open.last = t.tile;
      open.bytes += t.bytes;
    } else {
      runs.push({ first: t.tile, last: t.tile, start: t.start, bytes: t.bytes });
    }
  }
  return runs;
}

/** What one streamed question actually cost. Every number here is observed, not derived. */
export interface StreamCost {
  /** Tiles the window selected. */
  tiles: number;
  /** Tiles in the corpus. */
  ofTiles: number;
  /** HTTP requests issued — one per run. */
  requests: number;
  /** Bytes the runs asked for, from the footer's arithmetic. */
  askedBytes: number;
  /** Bytes the responses actually carried. Equal to `askedBytes` when the server honours
   *  `Range`; equal to the whole file when it does not, which is worth SEEING rather than
   *  hiding — see {@link StreamCost.ranged}. */
  gotBytes: number;
  /** False if any response came back `200` instead of `206`: the server ignored `Range` and
   *  sent the whole file. The demo's central claim is false on such a server and the panel
   *  must say so rather than quietly reporting the asked-for number. */
  ranged: boolean;
  /** Wall-clock milliseconds for the fetches. */
  ms: number;
}

/**
 * Fetch the runs, with one `Range` request each, and weigh what came back.
 *
 * `fetchImpl` is injected so `scripts/verify-stream.mjs` can drive this in Node against a
 * real server — the arithmetic above is worth checking against bytes, not against itself.
 */
export async function fetchRuns(
  url: string,
  runs: readonly Run[],
  totalTiles: number,
  selectedTiles: number,
  fetchImpl: typeof fetch = fetch,
): Promise<StreamCost> {
  const started = performance.now();
  let gotBytes = 0;
  let ranged = true;

  const bodies = await Promise.all(
    runs.map(async (run) => {
      const end = run.start + run.bytes - 1;
      const response = await fetchImpl(url, { headers: { Range: `bytes=${run.start}-${end}` } });
      if (!response.ok) throw new Error(`${url} answered ${response.status} for bytes=${run.start}-${end}`);
      // 206 is the honest answer. A 200 means the whole file arrived and the range was
      // ignored; the byte count below then reports the truth and `ranged` flags it.
      if (response.status !== 206) ranged = false;
      const buffer = await response.arrayBuffer();
      return buffer.byteLength;
    }),
  );
  for (const n of bodies) gotBytes += n;

  return {
    tiles: selectedTiles,
    ofTiles: totalTiles,
    requests: runs.length,
    askedBytes: runs.reduce((a, r) => a + r.bytes, 0),
    gotBytes,
    ranged,
    ms: performance.now() - started,
  };
}
