/**
 * The capabilities this package asks a host for — **run SQL and hand back rows, or, for a caller
 * that only wants addresses, read one text file.**
 *
 * {@link QueryFn} is the engine and it is the only thing the door needs. {@link ReadTextFn} buys
 * strictly less: it cannot answer a verb, cannot read a footer and cannot decode a byte of Parquet,
 * and it exists because the manifests are text and the engine-free route had no way to say so. See
 * its own doc for the measured reason it is a capability rather than a documented file name.
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

/**
 * The host's text reader: given an absolute URL, hand back what is at it.
 *
 * ```ts
 * const addressing = await openCorpus(base, {
 *   readText: async (url) => (await fetch(url)).text(),
 *   wasmUrl,
 * });
 * ```
 *
 * **It exists because the engine-free route could not say which files to read, and three readers
 * paid for that.** `openCorpus(base, { manifestFiles })` takes the manifests already in hand, and
 * to have them in hand you must know that the index is `graph.graph.yml` and that its `vertices:`
 * and `edges:` lists name the rest. That file name came off the barrel on the grounds that *«the
 * index's file name is the door's business»* — true of the engine-bearing route, which reads it
 * itself, and false of the other one, which was left to hand-write a twelve-line scan of the two
 * lists. `apps/playground/src/bench.ts` wrote it, `apps/playground/scripts/verify-canvas.mjs`
 * wrote it, and a third reader outside this repository wrote it again, which is the second
 * reference the rule names. Both copies here are deleted and pass this instead.
 *
 * **Why a capability and not an exported `GRAPH_INFO_PATH` plus a `manifestPathsOf(index)`.** That
 * pair is the same two facts published as data for the caller to re-assemble: it hands back a list
 * of paths, and every caller then writes the same join, the same fetch loop and the same error
 * when one is missing. The door already owns that sequence — it is step 1 of `openCorpus` — and
 * what the engine-free caller was missing was not the file name but the SEQUENCE. So it lends the
 * one thing it has that the package does not, and gets the sequence back. It is also what keeps
 * the index's file name off the surface, which was the right half of the original decision.
 *
 * **It is not a second `query`.** A host with an engine passes `query` and never this; a host with
 * neither passes `manifestFiles`. The three are one ladder — engine, fetcher, bytes in hand — and
 * `openCorpus` takes exactly one rung.
 *
 * Synchronous is accepted, for a host reading off a local disk (`readFileSync`) — which is what
 * every verifier script under `apps/playground/scripts` is.
 */
export type ReadTextFn = (url: string) => Promise<string> | string;
