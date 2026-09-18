/**
 * The large corpus, addressed — the half of streaming that costs nothing.
 *
 * `scripts/bench-corpus.mjs` writes a million-vertex corpus into this app's `public/` at build
 * time; this module opens it the way any reader would: lend `openCorpus` a `fetch`, and get back
 * every URL the corpus can produce.
 *
 * ## What addressing costs now
 *
 * It used to cost nothing: `@fossil-lang/corpus/address` was a subpath that imported a YAML
 * reader and no engine, and a test next door built the package with no `pkg/` at all to prove it.
 * **That subpath is gone**, and so is the claim. The addressing is `fossil_graph::plan` compiled
 * to wasm32 — one implementation instead of two — so composing a URL costs loading a wasm module
 * where it used to cost parsing three small YAML files.
 *
 * What survives is the shape of the win, and it is the half that mattered: **no engine and no
 * request**. Every tile URL in a million-vertex corpus becomes available on one line, out of the
 * manifests already in hand. The boot is an option on that line now — `wasmUrl`, the same option
 * `openCorpus` takes — rather than a call sequenced before it, and it is memoised for the rest of
 * the session.
 *
 * **The capability is what decides the depth, and this module is why that is a capability rather
 * than a preference**: there is no `query` here to give the door and nothing to run one against, so
 * `openCorpus` is handed a text reader and answers with the addressing alone. It was a second
 * function called `resolveCorpus`; it is the same name at a shallower depth now.
 *
 * **What this module no longer contains is the twelve-line scan of the index's `vertices:`/`edges:`
 * lists.** It had one, `scripts/verify-canvas.mjs` had the same one, and a third reader outside
 * this repository wrote it again — because the package published `manifestFiles` as a requirement
 * and nothing that said which files those are. `readText` is that sequence published: lend a
 * reader, and the package asks for the index and then for what the index names.
 */
import { openCorpus, type CorpusAddressing } from '@fossil-lang/corpus';

import { CORPUS_WASM_URL } from './corpus.js';

/** What `scripts/bench-corpus.mjs` recorded about the corpus it wrote. */
export interface BenchStamp {
  dir: string;
  count: number;
  edges: number;
  tiles: number;
  layout: 'files' | 'rowgroups';
  chunkSize: number;
  /** Milliseconds the generator took, so the panel can say what the demo cost to make. */
  ms: number;
  /** Total bytes of the corpus on disk — the denominator, measured at build time rather
   *  than by issuing requests for files this app is demonstrating that it does not fetch. */
  bytes: number;
  /** Every file and its size, largest first. */
  files: { path: string; bytes: number }[];
}

/** The corpus, opened for addressing. */
export interface Bench {
  stamp: BenchStamp;
  /**
   * Dataset root, as an **absolute** URL on this app's own origin. Nothing else is ever fetched.
   *
   * Absolute, and it used to be `bench/1000000` — dataset-relative, which is what the stamp
   * records and what a manifest's own prefixes are written against. That worked for everything
   * this module fetches, because `fetch` in a document resolves against the document. It does
   * not work for the one consumer that is not in the document: **DuckDB-WASM resolves a
   * registered URL inside its Worker**, where the base is the worker script rather than the
   * page, so `bench/1000000/vertex/Person/tiles.parquet` resolved to a path under Vite's
   * dependency-optimiser directory, came back as the SPA fallback's `index.html`, and the
   * footer read died with `No magic bytes found at end of file`. Anchoring here rather than at
   * each call site means every URL the addressing produces is one a Worker can open, which is
   * the property the addressing layer is supposed to be delivering.
   */
  base: string;
  addressing: CorpusAddressing;
  /**
   * `graph.graph.yml` as it was served, verbatim.
   *
   * Kept because addressing throws it away and one reader wants it whole: the `privacy:` block
   * is a claim about these bytes, and the panel that re-derives it shows the declaration beside
   * the recomputation. Costs nothing — the text is already in hand and already counted below.
   */
  indexText: string;
  /**
   * Every manifest as it was served, keyed by the dataset-relative path — the index under
   * `graph.graph.yml`, and one per type beside it.
   *
   * The same map the addressing was resolved from, kept for the reader that wants a document the
   * addressing does not: a vertex type's `channels:` block is per TYPE and lives in
   * `vertex/<Type>.vertex.yml`, so an encoding cannot get at it through `indexText` the way
   * `src/bound.ts` gets at `privacy:`. Costs nothing — these are the bytes already fetched and
   * already counted below.
   */
  manifestTexts: Record<string, string>;
  /** The manifests, and what they weighed — the only bytes addressing costs. */
  manifestBytes: number;
  manifestCount: number;
  /**
   * Milliseconds spent fetching manifests, and milliseconds spent addressing them.
   *
   * The two are separated by the `readText` callback, which is the only place a request happens:
   * whatever is not inside it is the boot and the arithmetic. `resolveMs` covers the reader's
   * one-time WASM boot as well, because the boot is an option on the call rather than a step
   * before it; it is memoised per session, so a second corpus in the same tab pays the arithmetic
   * alone — and neither number is a request for a byte of payload, which is the claim the panel
   * beside them makes.
   */
  fetchMs: number;
  resolveMs: number;
}

/** Where the build script leaves its record. Absent in a checkout where it never ran. */
const STAMP_URL = 'bench/index.json';

/**
 * Open the bench corpus for addressing, or return `null` if it was never generated.
 *
 * `null` is not an error: `pnpm dev` has to work in a checkout where `bench-corpus.mjs` has
 * not run, and a playground that refuses to boot because an optional 56 MB demo asset is
 * missing would be a worse app than one that says so in a sentence.
 *
 * **`ok` is not the test, and that is the whole of why this reads a header.** Vite's dev
 * server answers a missing path with the SPA fallback — `200`, and `index.html` in the body —
 * so `ok` is true for a stamp that does not exist and `json()` throws a `SyntaxError` the
 * caller renders as `failed`. The sentence this function exists to make possible was being
 * replaced by `Unexpected token '<'` in exactly the checkout it was written for.
 */
export async function openBench(): Promise<Bench | null> {
  const stampResponse = await fetch(STAMP_URL);
  if (!stampResponse.ok) return null;
  if (!(stampResponse.headers.get('content-type') ?? '').includes('json')) return null;
  const stamp = (await stampResponse.json()) as BenchStamp;
  // `document.baseURI` rather than `location.href`: the second carries the current path, so an
  // app served from anything but the root would resolve the dataset against the wrong directory.
  const base = new URL(stamp.dir, document.baseURI).href.replace(/\/+$/, '');

  // The whole of the addressing, on one call and with no engine. Every tile URL of a
  // million-vertex corpus becomes available here — the `wasmUrl` boots the reader (memoised), the
  // callback below is every request that happens, and nothing under it is a byte of payload.
  //
  // **Which files get asked for is not this module's decision any more.** It used to scan the
  // index's `vertices:`/`edges:` lists itself to find out; `openCorpus` knows, because the index
  // is its business, and all this lends it is a `fetch`.
  const manifestFiles: Record<string, string> = {};
  let fetchMs = 0;
  let indexText: string | null = null;
  let ungenerated = false;

  const started = performance.now();
  let addressing: CorpusAddressing;
  try {
    addressing = await openCorpus(base, {
      wasmUrl: CORPUS_WASM_URL,
      readText: async (url) => {
        const at = performance.now();
        const response = await fetch(url);
        // The FIRST file asked for is the index, and a 404 on it means this checkout never ran
        // `bench-corpus.mjs` — the `null` the doc above promises, and not a corpus that is wrong.
        if (!response.ok) {
          ungenerated = indexText === null;
          throw new Error(`${url} — ${response.status}`);
        }
        const text = await response.text();
        fetchMs += performance.now() - at;
        indexText ??= text;
        manifestFiles[url.slice(base.length + 1)] = text;
        return text;
      },
    });
  } catch (cause) {
    if (ungenerated) return null;
    throw cause;
  }
  const resolveMs = performance.now() - started - fetchMs;

  const manifestBytes = Object.values(manifestFiles).reduce((a, t) => a + new Blob([t]).size, 0);

  return {
    stamp,
    base,
    addressing,
    // Non-null by construction: `openCorpus` answered, so it read the index.
    indexText: indexText!,
    manifestTexts: manifestFiles,
    manifestBytes,
    manifestCount: Object.keys(manifestFiles).length,
    fetchMs,
    resolveMs,
  };
}
