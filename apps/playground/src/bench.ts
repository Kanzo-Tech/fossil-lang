/**
 * The large corpus, addressed — the half of streaming that costs nothing.
 *
 * `scripts/bench-corpus.mjs` writes a million-vertex corpus into this app's `public/` at build
 * time; this module opens it the way any reader would: fetch the manifests, hand them to
 * `resolveCorpus`, and get back every URL the corpus can produce.
 *
 * ## The import is the claim
 *
 * `@fossil-lang/graph/address`, not `@fossil-lang/graph`. The barrel needs the WASM verb
 * surface and static-imports `pkg/`; the addressing subpath imports nothing but a YAML reader.
 * That separation is enforced by `packages/graph/tests/address-standalone.test.ts`, which
 * builds the package with no `pkg/` at all and runs `resolveCorpus` from a consumer with an
 * otherwise empty `node_modules`. So "computing the URLs needs no engine" is not a claim this
 * app makes about itself — it is a property of the module it imports, tested next door.
 *
 * `resolveCorpus` is synchronous. Everything asynchronous here is fetching the three small
 * YAML files it reads; once they are in hand, every tile URL in a million-vertex corpus is
 * available without another request.
 */
import { resolveCorpus, type CorpusAddressing } from '@fossil-lang/graph/address';

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
  /** Dataset root, relative to this app's origin. Nothing else is ever fetched. */
  base: string;
  addressing: CorpusAddressing;
  /** The manifests, and what they weighed — the only bytes addressing costs. */
  manifestBytes: number;
  manifestCount: number;
  /** Milliseconds spent fetching manifests, and milliseconds inside `resolveCorpus`. */
  fetchMs: number;
  resolveMs: number;
}

/**
 * Pull the manifest paths out of the index.
 *
 * A three-line reader for two list keys rather than a YAML dependency: `resolveCorpus` needs
 * the manifest FILES, and to know which files those are you have to read the index's
 * `vertices:` and `edges:` lists first. `openCorpus` does exactly this and for exactly this
 * reason. Anything subtler about the manifests is `resolveCorpus`'s job, not this function's —
 * it is looking for filenames, and a filename that is not there produces a
 * `CorpusManifestError` from the real reader, which is a better error than one invented here.
 */
function manifestPaths(index: string): string[] {
  const paths: string[] = [];
  let inList = false;
  for (const raw of index.split('\n')) {
    if (/^(vertices|edges):\s*$/.test(raw)) {
      inList = true;
      continue;
    }
    const item = /^-\s+(\S+)\s*$/.exec(raw);
    if (inList && item) paths.push(item[1]!);
    else if (!/^\s*$/.test(raw) && !item) inList = false;
  }
  return paths;
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
  const base = stamp.dir;

  const started = performance.now();
  const index = await fetch(`${base}/graph.graph.yml`);
  if (!index.ok) return null;
  const indexText = await index.text();

  const paths = manifestPaths(indexText);
  const rest = await Promise.all(
    paths.map(async (path) => [path, await (await fetch(`${base}/${path}`)).text()] as const),
  );
  const fetchMs = performance.now() - started;

  const manifestFiles: Record<string, string> = { 'graph.graph.yml': indexText };
  for (const [path, text] of rest) manifestFiles[path] = text;

  // The whole of the addressing, and it is synchronous. Every tile URL of a million-vertex
  // corpus becomes available on this line, with no engine loaded and no further request.
  const resolveStarted = performance.now();
  const addressing = resolveCorpus({ manifestFiles, base });
  const resolveMs = performance.now() - resolveStarted;

  const manifestBytes = Object.values(manifestFiles).reduce((a, t) => a + new Blob([t]).size, 0);

  return {
    stamp,
    base,
    addressing,
    manifestBytes,
    manifestCount: Object.keys(manifestFiles).length,
    fetchMs,
    resolveMs,
  };
}
