/**
 * Types for `guards.mjs`, for the one TypeScript consumer there is.
 *
 * `guards/` is plain ESM with no build step, because it is meant to be copied by somebody who has
 * neither this repository nor a bundler. `components/guard-index.tsx` renders the contract from it
 * at build time, and this file is what lets it do that without a `@ts-expect-error` — which would
 * be a directive that silently stops meaning anything the day the resolution changes.
 *
 * Only what the site renders is declared. The checker's own surface is larger and is documented in
 * the module.
 */

/** One convention, as an executable check. */
export interface Guard {
  /** Stable id, used by `check.mjs --only` and printed on failure. */
  id: string;
  title: string;
  /** What holds of a corpus that passes. */
  proves: string;
  /** What still may not hold. The price of documenting a format instead of typing it. */
  cannotProve: string;
  run(corpus: unknown): { failures: string[]; notes: string[] };
}

export declare const GUARDS: Guard[];

export declare function checkVectors(vectors: unknown): { failures: string[]; notes: string[] };

export declare function runAll(
  corpus: unknown,
  only?: string[] | null,
): Array<{ guard: Guard; failures: string[]; notes: string[]; ms: number }>;
