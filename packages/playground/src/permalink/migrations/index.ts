/**
 * Permalink migration registry. Each future migration function transforms a
 * v(N) payload into a v(N+1) payload.
 *
 * Currently SCHEMA_VERSION=1 so there are no migrations. This file exists so:
 *   1. The import path is stable — when v2 lands, the registry table moves
 *      here without touching `envelope.ts`'s exports.
 *   2. `git log packages/playground/src/permalink/migrations/` documents
 *      schema evolution as a flat history (one PR per migration).
 *   3. PROJECT.md's "Permalinks v0.2 only ships if/when needed" decision has
 *      a concrete code anchor so future contributors find the migration
 *      pattern by directory name.
 *
 * The actual migration table lives in `../envelope.ts` for now — keeping it
 * one file shorter while v=1. When v=2 ships, move the table here and
 * re-export from envelope.ts to preserve back-compat for any external
 * consumers that imported the symbol.
 */

export {};
