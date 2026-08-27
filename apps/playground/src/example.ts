/**
 * The walking skeleton, imported rather than copied.
 *
 * `examples/hello.fossil` + `examples/users.csv` + `examples/hello.shex` are the three
 * files `crates/fossil-cli/tests/walking_skeleton.rs` asserts by CONTENT — five rows of
 * CSV in, five `Person` vertices out, subjects `https://example.org/user/1`…`/5`. The
 * whole point of this app is that the browser produces the same five, so it reads the
 * same bytes: these are `?raw` imports from outside the app, not a copy under `src/`.
 * A copy would be a second source, and the first time the example changed the demo
 * would quietly stop being the demo.
 *
 * `vite.config.ts` allows the repo root in `server.fs` for exactly this.
 */
import helloFossil from '../../../examples/hello.fossil?raw';
import helloShex from '../../../examples/hello.shex?raw';
import usersCsv from '../../../examples/users.csv?raw';

/**
 * The paths the compiler sees.
 *
 * `hello.fossil` says `io.shex("hello.shex")` and `io.csv("users.csv")`, and every path a
 * program names resolves against THE PROGRAM FILE, never a working directory. So the two
 * siblings are opened under bare names beside it, which is what makes those two strings
 * resolve. Change these and the program stops finding its own documents.
 */
export const PROGRAM_PATH = 'hello.fossil';
export const SHEX_PATH = 'hello.shex';
export const CSV_PATH = 'users.csv';

export const PROGRAM = helloFossil;
export const SHEX = helloShex;
export const CSV = usersCsv;

/**
 * The base every relative source path is resolved against before the RUN. See
 * {@link absolutise}.
 *
 * `.invalid` is reserved by RFC 2606 and resolves nowhere, which is the point: it is
 * proof rather than promise that nothing is fetched. The bytes are already in memory and
 * the executor's object store is keyed by this scheme+authority, never dereferenced.
 */
export const SOURCE_BASE = 'https://playground.invalid/';

/**
 * The source bytes, keyed by the URI the program writes.
 *
 * `FossilExecutor.sources()` hands back those URIs and the host stages the bytes for each.
 * In a real host this is a `fetch` of a signed URL; here it is a build-time import, which
 * is the strongest possible version of the privacy claim — there is no request to make.
 */
export const CSV_BYTES: Uint8Array = new TextEncoder().encode(usersCsv);

/**
 * Keyed by the ABSOLUTE URI, because that is what `sources()` reports after
 * {@link absolutise} — and the descriptor the checker gets is keyed by the RELATIVE one,
 * because the checker only ever sees what the program says. Two keys, one buffer, and the
 * difference between them is exactly the seam `absolutise` documents.
 */
export const SOURCE_BYTES: Record<string, Uint8Array> = {
  [`${SOURCE_BASE}${CSV_PATH}`]: CSV_BYTES,
};

/**
 * Make every relative `io.` source URI absolute, for the executor only.
 *
 * **This is a seam, and it is the executor's, not a shortcut here.**
 * `fossil-df-wasm`'s `register_object_store_sources` does `Url::parse(&src.uri)` and keys
 * an `InMemory` store by the URI's scheme+authority. A relative path has neither, so it
 * fails with «relative URL without a base» before any plan runs. Natively there is no such
 * problem: `fossil-cli` resolves every path a program names against THE PROGRAM FILE, so
 * the executor only ever sees `file:///…`. In a browser there is no program file to
 * resolve against, and nothing in the wasm surface does the resolving.
 *
 * So the app does it, on the program text, and only on the way into `run` — the CHECKER
 * still sees the program exactly as written, which is what keeps the diagnostics honest.
 * `io.shex` is deliberately not rewritten: the shape document reaches the executor as a
 * separate argument, not as an object-store source.
 *
 * The right fix is upstream — either the wasm host takes a base URI, or
 * `register_object_store_sources` treats a relative URI as relative to `dest`. Either is a
 * crate change, and this app is not the place for it.
 */
export function absolutise(program: string): string {
  return program.replace(/\bio\.(csv|json|parquet)\("([^"]+)"\)/g, (whole, row: string, uri: string) =>
    /^[a-z][a-z0-9+.\-]*:/i.test(uri) ? whole : `io.${row}("${SOURCE_BASE}${uri}")`,
  );
}

/** Where the run writes. A URL because `RunReport.dest` is one; nothing fetches it. */
export const DEST = 'memory://corpus';
