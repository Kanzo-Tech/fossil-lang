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
 */
export interface Engine {
  query(sql: string): Promise<Record<string, unknown>[]>;
  /**
   * Make each URL readable under its name. The same URL again is a no-op and a new URL
   * replaces the lease behind the name — DuckDB-WASM's `registerFileURL` refuses a second URL
   * for a name, so this is the engine's to reconcile and never the caller's.
   */
  lend(files: Record<string, string>): Promise<void>;
  /** Forget the names. A name the engine does not hold is ignored. */
  drop(names: readonly string[]): Promise<void>;
}
