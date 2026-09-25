import type { SourceHost } from './source-host';

/**
 * The page's one SQL engine, as fossil needs it — a DuckDB-WASM with a file registry.
 *
 * Fossil does not own it and does not boot one: `@kanzo-tech/mosaic`'s `engine()` is the
 * reference and satisfies this structurally. A corpus or an introspection names files here
 * and never puts a signed URL into SQL text, so a signature cannot reach an error message, a
 * query-cache key or a view definition, and it can rotate under a view that stays.
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

/**
 * What a host gives a reader that only needs URLs signed — a job's output, which has no
 * connection map. `ttlMs` is how long a signature lives; given, the reader signs again
 * before it lapses, and absent, a signature is taken to last as long as the reader.
 */
export type Signer = Pick<SourceHost, 'sign'> & { readonly ttlMs?: number };
