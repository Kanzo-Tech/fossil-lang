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
 * THE SEAM THAT WAS NOT REAL, AND IS
 * ═══════════════════════════════════════════════════════════════════════════════════
 *
 * This file used to carry a `retile()` helper and a long note calling it «the one thing
 * in this app that is not real». `fossil-df-wasm` did not run the layout pass, so what
 * `FossilExecutor.run` handed back was the STAGED tree — one file per vertex type at
 * `vertex/Person.parquet` — while the manifest beside it declared `vertex/Person/` and an
 * `index/`, the tree the layout pass DELIVERS. `retile()` papered over the difference with
 * a DuckDB `COPY … ROW_GROUP_SIZE`, which produced row groups of the right size and
 * nothing else: no Louvain communities, no Morton order, `x`/`y`/`cluster_id` still at the
 * writer's zeros.
 *
 * That was not a cosmetic gap. The corpus format's whole addressing argument is that
 * `dense_id` ascends with the Morton code of the vertex's position — the id space IS the
 * spatial order, which is what makes a rectangle break into O(√n) contiguous runs and a
 * window a range of tiles (`/docs/format/conventions/addressing`). A corpus with zeroed
 * coordinates satisfies the manifest's shape and violates the property the shape exists to
 * express, and every count-based check passes while it does.
 *
 * The note ended «This file should be deleted, not extended. The fix is a wasm-bindgen
 * binding over `fossil-layout` … called from `fossil-df-wasm` after `execute_graph`, so
 * that the browser and the CLI write the same tree.» That is what happened:
 * `crates/fossil-layout/src/io.rs` makes the pass's filesystem a parameter and
 * `fossil-df-wasm` drives it through `MemoryFs` after `execute_graph`. **The executor now
 * hands back the tiled tree**, so this file stages bytes and opens them, and there is no
 * stand-in left to document.
 */
import { openCorpus, type Corpus } from '@fossil-lang/graph';
import type { GraphArFile } from '@fossil-lang/executor';

import { query, register } from './duckdb.js';

/** Where the corpus is addressed from. Any prefix works; it just has to be consistent. */
export const CORPUS_URL = 'corpus';

/** A single-quoted SQL string literal. Every path in this module reaches SQL through here. */
const lit = (value: string) => `'${value.replace(/'/g, "''")}'`;

/**
 * Stage every file the run produced under the URL the manifest is addressed from.
 *
 * These are the layout pass's OUTPUT — `vertex/<Type>/tiles.parquet`, its `index/`, and
 * `by_source`/`by_target` tiles per edge type — because the pass ran inside the executor.
 * Nothing is rewritten here; the bytes are registered at the exact names `openCorpus`
 * composes by arithmetic.
 */
export async function stage(files: readonly GraphArFile[]): Promise<void> {
  for (const file of files) {
    await register(`${CORPUS_URL}/${file.path}`, file.bytes);
  }
}

/** Open the corpus through the reference door. Throws if the manifest cannot address itself. */
export function open(): Promise<Corpus> {
  return openCorpus(CORPUS_URL, { query });
}

/**
 * The subjects, read back the way the walking-skeleton test reads them.
 *
 * It reads the TILED payload — `vertex/<Type>/tiles.parquet` — because that is the only
 * vertex payload the corpus has now. It used to read the staged single file and to
 * `EXCLUDE (x, y, cluster_id)` on the grounds that «a window needs coordinates, the
 * coordinates are the layout pass's output, and the layout pass did not run». The pass runs,
 * so the columns carry a real placement and the exclusion would now be hiding the evidence
 * that it did. `dense_id` order is Morton order here, which is the property
 * `/docs/format/conventions/addressing` is about.
 */
export function subjectsQuery(type: string): string {
  return (
    `SELECT dense_id, subject, x, y, cluster_id, ` +
    `* EXCLUDE (dense_id, subject, x, y, cluster_id) ` +
    `FROM read_parquet(${lit(`${CORPUS_URL}/vertex/${type}/tiles.parquet`)}) ` +
    `ORDER BY dense_id`
  );
}
