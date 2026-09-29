/**
 * The page's one SQL engine, as fossil needs it — a DuckDB-WASM with `httpfs` loaded and a file
 * registry.
 *
 * Fossil does not own it and does not boot one: `@kanzo-tech/mosaic`'s `engine()` is the reference
 * and satisfies this structurally. It is the database, not the reader: fossil puts a vended
 * credential into it as a scoped `CREATE SECRET` and names `s3://…` in SQL, so a credential never
 * reaches SQL text, an error message, a query-cache key or a view definition, and it rotates under
 * a view that stays. `lend` and `drop` are for what a secret cannot reach — an Azure file, which
 * DuckDB-WASM has no extension for, and bytes the page already holds.
 *
 * **A host should cache Parquet metadata** — `SET parquet_metadata_cache = true`, once, when it
 * boots. A corpus's payload is one file per type with a row group per tile, so its footer lists
 * every tile and grows with the corpus, and without the cache every statement fetches and parses it
 * again: a tile read costs the corpus rather than the tile. Measured in `/docs/design/backend`, it
 * is 2.2× on a window's reads in DuckDB-WASM and 2.5× over a 10 ms link.
 */
export interface Engine {
  /**
   * Run one statement and answer in columns.
   *
   * **The signal reaches the running statement, not a queue.** A DuckDB-WASM connection runs one
   * statement at a time, so an abort that only dropped queued work would leave the one that matters
   * — the running, stale one — to finish. An engine interrupts it (`AsyncDuckDBConnection.send`
   * then `cancelSent()` in DuckDB-WASM, `interrupt()` on a native connection) and rejects with the
   * signal's reason, an `AbortError`; the connection answers the next statement as if the aborted
   * one had never been sent.
   *
   * **Columns, because a reader draws columns.** apache-arrow's `Table` is a {@link Table}
   * structurally, so a host hands back what DuckDB-WASM already produced and nothing turns columns
   * into objects and back.
   */
  query(sql: string, options?: { readonly signal?: AbortSignal }): Promise<Table>;
  /**
   * Make each URL readable under its name. The same URL again is a no-op and a new URL
   * replaces the lease behind the name — DuckDB-WASM's `registerFileURL` refuses a second URL
   * for a name, so this is the engine's to reconcile and never the caller's.
   */
  lend(files: Record<string, string>): Promise<void>;
  /** Forget the names. A name the engine does not hold is ignored. */
  drop(names: readonly string[]): Promise<void>;
}

/**
 * An answer, in columns — the part of apache-arrow's `Table` fossil reads, so an Arrow table is one
 * without a conversion and without this package depending on Arrow.
 */
export interface Table {
  readonly numRows: number;
  readonly schema: { readonly fields: readonly { readonly name: string }[] };
  /** One column by name, or `null` when the answer has none of that name. */
  getChild(name: string): Column | null;
}

/**
 * One column of a {@link Table}. `get` answers `null` for a null; `toArray` is the column's own
 * array — a typed array for a fixed-width type, in which a null reads as that type's zero.
 */
export interface Column {
  readonly length: number;
  get(index: number): unknown;
  toArray(): ArrayLike<unknown>;
}
