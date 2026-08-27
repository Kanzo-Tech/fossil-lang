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
 * The source bytes, keyed by the URI the program writes.
 *
 * `FossilExecutor.sources()` hands back those URIs and the host stages the bytes for each.
 * In a real host this is a `fetch` of a signed URL; here it is a build-time import, which
 * is the strongest possible version of the privacy claim — there is no request to make.
 */
export const SOURCE_BYTES: Record<string, Uint8Array> = {
  [CSV_PATH]: new TextEncoder().encode(usersCsv),
};

/** Where the run writes. A URL because `RunReport.dest` is one; nothing fetches it. */
export const DEST = 'memory://corpus';
