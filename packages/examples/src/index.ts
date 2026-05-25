/**
 * @fossil-lang/examples — bundled fixture examples for the playground.
 *
 * v0.1 shipped a single example (`hello`); Phase 9 PLAY-05 grew this to a
 * curated set of 6 examples + a `variations/` set per example (1 base + 4-5
 * mutations) verified in CI by the variations harness.
 *
 * All example contents are inlined at consumer-bundle time via Vite `?raw`
 * imports (see `src/types.d.ts` for the ambient module shim). This keeps the
 * package tree-shakeable and OFFLINE-01 compatible — no separate `fetch()`s
 * needed for fixture files.
 */

/**
 * One data file referenced by an example's mapping via an `@examples/...`
 * connector path. The `path` is what comes after `@examples/` in the
 * SourceRef; the `contents` is the raw string handed to the resolver.
 */
export interface ExampleFile {
  /** Logical path inside the `@examples/*` connector (no leading slash). */
  path: string;
  /** File contents (string). */
  contents: string;
  /** Format hint for the resolver's `ResolvedSource.format` field. */
  format?: 'csv' | 'json' | 'parquet';
}

/**
 * A single bundled example — the unit the React playground mounts as the
 * initial editor content + the resolver-served fixture set.
 */
export interface Example {
  /** Stable identifier (URL slug-safe). Used as the resolver key prefix. */
  id: string;
  /** Display title for the examples gallery. */
  title: string;
  /** One-line description (~80 chars) for the examples gallery. */
  description: string;
  /** The `.fossil` source — loads into the editor by default. */
  mapping: string;
  /** Optional CSVW JSON-LD descriptor source. */
  csvw?: string;
  /** Optional ShEx target schema. */
  shex?: string;
  /** Data files referenced by the mapping via `@examples/...` paths. */
  dataFiles: ExampleFile[];
}

import { helloExample } from './hello/index.js';
import { helloNoCsvwExample } from './hello-no-csvw/index.js';
import { ecommerceExample } from './ecommerce/index.js';
import { musicbrainzExample } from './musicbrainz/index.js';
import { typingShowcaseExample } from './typing-showcase/index.js';
import { multiSourceJoinExample } from './multi-source-join/index.js';
import manifest from './manifest.json';

export {
  helloExample,
  helloNoCsvwExample,
  ecommerceExample,
  musicbrainzExample,
  typingShowcaseExample,
  multiSourceJoinExample,
};

/**
 * All bundled examples — ordered to match {@link EXAMPLES_MANIFEST} for
 * stable dropdown enumeration in the playground UI.
 */
export const examples: Example[] = [
  helloExample,
  helloNoCsvwExample,
  ecommerceExample,
  musicbrainzExample,
  typingShowcaseExample,
  multiSourceJoinExample,
];

/**
 * Declarative manifest of curated examples (PLAY-05). The shape mirrors
 * `manifest.json` on disk — consumed by the playground UI for dropdown
 * enumeration, by the variations harness for path resolution, and by the
 * landing app's `<ExampleSelector/>`.
 */
export const EXAMPLES_MANIFEST = manifest;

/** Union of legal example ids drawn from the manifest. */
export type ExampleId = (typeof manifest)['examples'][number]['id'];

/**
 * Build the `examples` map argument for `createDefaultResolver({ examples })`
 * (from `@fossil-lang/resolvers`).
 *
 * Keyed on the path that comes after `@examples/` in a SourceRef. The mapping
 * itself is NOT included (it goes into the editor, not the resolver). Data
 * files + descriptors are connector-resolvable under their full path.
 *
 * Example output for the v0.1 hello bundle:
 * ```
 * {
 *   "hello.csv":       "id,name\n1,Alice\n...",
 *   "hello.csvw.json": "{ \"@context\": ... }",
 *   "hello.shex":      "PREFIX ex: <https://example.org/> ..."
 * }
 * ```
 */
export function buildResolverExamples(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const ex of examples) {
    for (const df of ex.dataFiles) {
      out[df.path] = df.contents;
    }
    if (ex.csvw) out[`${ex.id}.csvw.json`] = ex.csvw;
    if (ex.shex) out[`${ex.id}.shex`] = ex.shex;
  }
  return out;
}
