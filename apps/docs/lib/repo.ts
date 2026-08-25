import { resolve } from "node:path";

/**
 * The repository root, as seen from this app.
 *
 * Every path a page cites, and every `src` a `<Program>` transcludes, is written relative to the
 * repo root rather than to this app, because that is how `git log` writes them. A path that means
 * something different depending on who reads it is a path nobody will keep up to date.
 *
 * `process.cwd()` is `apps/docs` for `next build`, `next dev` and `vitest run` alike, which is what
 * lets the site renderer and the guard test agree on where they are without a second mechanism.
 */
export const repoRoot = resolve(process.cwd(), "../..");
