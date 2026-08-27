/**
 * Half three of the loop: query the corpus that was just written.
 *
 * `openCorpus(url, { query })` is the door — one argument is the corpus and the other is
 * the engine, and everything else is read off the artefact. It is used here exactly as a
 * reader over HTTP would use it, against a `url` whose files happen to live in DuckDB's
 * virtual filesystem instead of on a server. The reader cannot tell, and that is the
 * demonstration: the same tile arithmetic, the same manifest, the same SQL.
 *
 * ═══════════════════════════════════════════════════════════════════════════════════
 * THE SEAM, AND IT IS THE ONE THING IN THIS APP THAT IS NOT REAL
 * ═══════════════════════════════════════════════════════════════════════════════════
 *
 * `fossil-df-wasm` does not run the layout pass. `crates/fossil-layout` is a normal
 * dependency of `fossil-cli` and of nothing on the browser path, so what
 * `FossilExecutor.run` hands back is the STAGED tree — one file per vertex type at
 * `vertex/Person.parquet` — while the manifest it hands back alongside declares
 * `prefix: vertex/Person/` and an `index/`, which is the tree the layout pass DELIVERS.
 * `crates/fossil-df/src/lib.rs` says so in as many words: *"the manifest is the plan"*,
 * declared "beside the properties that make it possible, rather than after the layout
 * pass that fills it".
 *
 * So natively, `fossil run` writes the staged tree and then re-tiles it into the
 * manifest-declared prefix. In the browser nothing does, and `openCorpus` — which
 * computes every tile URL before its first request, by arithmetic, exactly as designed —
 * asks for `corpus/vertex/Person/chunk0.parquet` and finds nothing there.
 *
 * {@link retile} is a stand-in for that pass, in SQL, and it is a STAND-IN: it slices by
 * `dense_id` into the declared `chunk_size` and orders the index by `subject`, and it does
 * NOT do what `fossil-layout` is actually for — Louvain communities and a Morton ordering,
 * which is what makes `x`/`y` mean anything and what makes a windowed read skip tiles.
 * The columns are all present and all zero (`fossil-df` writes `x=0, y=0, cluster_id=0` as
 * placeholders), so `extent()` returns a degenerate point and `window()` returns everything
 * or nothing depending on whether the box contains the origin. The five subjects are real;
 * the geometry is not.
 *
 * **This file should be deleted, not extended.** The fix is a wasm-bindgen binding over
 * `fossil-layout` — which already compiles for wasm32 and declares
 * `[package.metadata.fossil] wasm = true` — called from `fossil-df-wasm` after
 * `execute_graph`, so that the browser and the CLI write the same tree. That is a crate
 * change and this app is not the place for it.
 */
import { openCorpus, type Corpus } from '@fossil-lang/graph/corpus';
import type { GraphArFile, RunReport } from '@fossil-lang/executor';

import { query, register } from './duckdb.js';

/** Where the corpus is addressed from. Any prefix works; it just has to be consistent. */
export const CORPUS_URL = 'corpus';

/** A single-quoted SQL string literal. Every path in this module reaches SQL through here. */
const lit = (value: string) => `'${value.replace(/'/g, "''")}'`;

/** Stage every file the run produced under the URL the manifest is addressed from. */
export async function stage(files: readonly GraphArFile[]): Promise<void> {
  for (const file of files) {
    await register(`${CORPUS_URL}/${file.path}`, file.bytes);
  }
}

/**
 * The stand-in for the layout pass. See the seam note at the top of this file.
 *
 * For each vertex type the manifest declares, slice the staged Parquet into
 * `<prefix>chunk{k}.parquet` by `dense_id` range, and write `index/tile{k}.parquet`
 * ordered by `subject`. Both land in DuckDB's virtual filesystem, so they are addressable
 * by the names `openCorpus` computes without anything being written to disk or fetched.
 *
 * Edges are not re-tiled. The walking skeleton has none, and doing it wrong for a demo
 * that cannot exercise it would be two stand-ins instead of one.
 */
export async function retile(report: RunReport): Promise<void> {
  for (const vertex of report.vertices) {
    const staged = `${CORPUS_URL}/vertex/${vertex.type}.parquet`;
    const chunk = BigInt(vertex.chunk_size);
    const count = BigInt(vertex.vertex_count);
    const tiles = chunk === 0n ? 0n : (count + chunk - 1n) / chunk;

    for (let k = 0n; k < tiles; k += 1n) {
      const lo = k * chunk;
      const hi = lo + chunk;
      const target = `${CORPUS_URL}/${vertex.prefix}chunk${k}.parquet`;
      await query(
        `COPY (SELECT * FROM read_parquet(${lit(staged)}) ` +
          `WHERE dense_id >= ${lo} AND dense_id < ${hi}) ` +
          `TO ${lit(target)} (FORMAT PARQUET)`,
      );
      // The index is the same rows in `subject` order — a second copy of the type, which
      // is what makes `corpus.node(iri)` one tile read instead of a scan.
      const indexTarget = `${CORPUS_URL}/${vertex.prefix}index/tile${k}.parquet`;
      await query(
        `COPY (SELECT * FROM read_parquet(${lit(staged)}) ORDER BY subject ` +
          `LIMIT ${chunk} OFFSET ${lo}) TO ${lit(indexTarget)} (FORMAT PARQUET)`,
      );
    }
  }
}

/** Open the corpus through the reference door. Throws if the manifest cannot address itself. */
export function open(): Promise<Corpus> {
  return openCorpus(CORPUS_URL, { query });
}

/**
 * The five subjects, read back the way the walking-skeleton test reads them.
 *
 * This is the equivalence the whole app exists to show, so it is a plain scan of the
 * staged payload and NOT a `corpus.window()` — a window needs coordinates, the coordinates
 * are the layout pass's output, and the layout pass did not run. Reading the subjects does
 * not need geometry, so it does not pretend to have any.
 */
export function subjectsQuery(type: string): string {
  return (
    `SELECT dense_id, subject, * EXCLUDE (dense_id, subject, x, y, cluster_id) ` +
    `FROM read_parquet(${lit(`${CORPUS_URL}/vertex/${type}.parquet`)}) ORDER BY dense_id`
  );
}
