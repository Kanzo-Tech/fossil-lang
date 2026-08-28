/**
 * The one capability this package asks a host for: run SQL, hand back rows.
 *
 * It lived in `client.ts` as the verb surface's `QueryFn`, reachable only through a barrel that
 * static-imports the wasm-bindgen output. {@link openCorpus} needs the same thing and must not need
 * the WASM, so the type moved here — one spelling, two callers, and `client.ts` re-exports it so
 * nothing downstream changed name.
 *
 * **Why a callback and not a decoder.** Three of the four members of the corpus API have to decode
 * Parquet, and this package has zero runtime dependencies. Bundling a decoder would make it a
 * driver; injecting one makes it a seam. Both hosts already have the engine: DuckDB-WASM in the
 * browser (`tests/e2e.test.ts` boots it) and a native `duckdb::Connection` on the server
 * (`crates/fossil-mcp/src/executor.rs`'s `ConnectionExecutor`). And an engine buys more than
 * decoding: DuckDB prunes Parquet row groups off the footer statistics by itself, which is the one
 * piece of the read path that would otherwise have to be written twice.
 *
 * **One method, and that is deliberate.** A capability with five methods is a driver. Everything
 * the corpus API needs — the manifest YAMLs (`read_text`), the per-tile boxes (`parquet_metadata`)
 * and the payload (`read_parquet`, over a list of paths in one call) — is SQL over the same
 * filesystem layer, so a host that can reach the tiles can already reach the rest. That is measured
 * rather than assumed: `@duckdb/duckdb-wasm@1.32.0` bundles DuckDB v1.4.3 and answers all three
 * against local paths under `NODE_RUNTIME`, which is what `tests/corpus.test.ts` runs on, and the
 * `duckdb` binary the corpus guards use answers them too. **In particular there is no separate
 * `fetch` capability**, because there is nothing a separate one could reach that the engine cannot:
 * it has to see the tiles or it cannot answer a window.
 *
 * **Three things it deliberately does not carry**, each because a host would have to lie to
 * provide it:
 *
 * - **No cancellation.** A camera that moves fast supersedes its own windows, so an `AbortSignal`
 *   is the obvious fifth parameter. Neither real caller can honour one: DuckDB-WASM's blocking node
 *   bindings run a query to completion, and `ConnectionExecutor::run` is synchronous Rust wrapped
 *   in a ready future. A seam that accepted a signal and ignored it would be worse than one that
 *   does not have it. Supersede-cancellation stays in the reader, where it can drop an answer.
 * - **No parameter binding.** Every value reaches SQL as a literal, so the caller of this callback
 *   owns quoting — `corpus.ts` routes every URL, IRI and column name through one escaper each, and
 *   every id through `BigInt`'s own decimal rendering. Binding would be a second method and a
 *   second dialect question; one string is what both hosts already accept.
 * - **No streaming.** The rows are materialised as an array, so a window over a dense region is in
 *   JS memory all at once. A cursor or an Arrow table would fix that and would put a shape from
 *   somebody else's library in the seam — which is the dependency this package does not take. The
 *   bound belongs in the box the caller asks for.
 */

/** One row of a result, as a plain `{ column: value }` object. */
export type QueryRow = Record<string, unknown>;

/**
 * The host's query callback. Runs SQL and resolves the rows as plain objects.
 *
 * In keasy this wraps the Mosaic coordinator, e.g.:
 *
 * ```ts
 * const query: QueryFn = async (sql) => {
 *   const table = await coordinator.query(sql, { type: 'arrow' });
 *   return table.toArray().map((r) => r.toJSON());
 * };
 * ```
 *
 * (Mosaic returns an Arrow table; the binding's WASM core expects row objects, so the host adapts
 * once here — keeping this package free of an Arrow/Mosaic dependency.)
 *
 * **What a value in a row may be is not narrowed, and that is not laziness.** The two hosts already
 * disagree about the width they hand back: DuckDB-WASM gives a `UINTEGER` as a `Number` and a
 * `UBIGINT` as a `BigInt`, and `ConnectionExecutor` turns both into JSON numbers. So a `dense_id`
 * arrives as a `number`, a `bigint` or a string depending on the host and the column's declared
 * width, and the corpus API coerces it at the boundary rather than trusting any of them.
 */
export type QueryFn = (sql: string) => Promise<QueryRow[]>;
