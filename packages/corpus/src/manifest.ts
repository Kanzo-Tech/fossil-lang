/**
 * The aggregate index, scanned for the one thing a host has to know before it can fetch anything.
 *
 * A corpus cannot be discovered by listing a directory — over HTTP there is no listing — so it has
 * one entry point, `graph.graph.yml`, and everything else is reached from there. `open`
 * fetches that file, and this is what tells it which per-type manifests to fetch next. That is the
 * whole job: **the manifests themselves are read by `fossil-graph`**, through
 * `fossil-graph-wasm`'s `Corpus`, and this module reads none of them.
 *
 * **It used to read all of them**, because the addressing was a second implementation of the
 * addressing and needed every field the Rust reader needs — a chunk size, an `index:` block, a
 * level list. That reader is gone, so the accessors it wanted are gone with it and the scanner is
 * narrowed to the shape its one remaining input has: a flat mapping of scalars, plus the two
 * sequences of paths this exists to return.
 *
 * **A line scanner and not a YAML parser, deliberately.** `apps/corpus/guards/manifest.mjs` makes
 * the same call and cannot be imported — it has no `package.json` and exists to be *copied* by a
 * third party who has neither this repository nor npm.
 *
 * **What it refuses rather than guesses:** anchors and aliases, flow style (`[a, b]`), and
 * multi-line scalars (`|`, `>`). A manifest using them throws with the line named, which is the
 * honest outcome — a scanner that silently reads half a document composes half the URLs and reports
 * none of it. A nested mapping (`privacy:`) is *skipped* rather than refused: it is legal, no
 * address is composed from it, and refusing it would fail on a corpus this reads fine.
 */

/** Dataset-relative location of the aggregate index. The one path a reader is told. */
export const GRAPH_INFO_PATH = 'graph.graph.yml';

/** One scanned manifest file: scalars, and sequences of scalars. */
export type ScannedManifest = Record<string, string | string[]>;

/** Raised by everything in this module, and by the reader behind the addressing. */
export class CorpusManifestError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CorpusManifestError';
  }
}

/** Strip the quoting serde emits around a string that needs it. */
function unquote(value: string): string {
  const v = value.trim();
  if (v.length >= 2 && ((v[0] === "'" && v.at(-1) === "'") || (v[0] === '"' && v.at(-1) === '"'))) {
    return v.slice(1, -1).replace(/''/g, "'");
  }
  return v;
}

const REFUSED: Array<[RegExp, string]> = [
  [/^\s*[-\w]+:\s*[|>][-+\d]*\s*$/, 'a multi-line scalar'],
  // `[]` and `{}` are how an empty collection is written and carry nothing to misread; a *populated*
  // flow collection is a second grammar for a sequence and this scanner has one.
  [/^\s*[-\w]+:\s*[[{]\s*[^\s\]}]/, 'flow style'],
  [/^\s*[-\w]*:?\s*[&*]\w/, 'an anchor or alias'],
];

/** Scan one manifest file. `path` is used only to name the file in an error. */
export function scan(path: string, text: string): ScannedManifest {
  const out: ScannedManifest = {};
  let sequence: string[] | null = null;

  for (const [index, raw] of text.split('\n').entries()) {
    const line = raw.replace(/\s+$/, '');
    if (line === '' || line.trimStart().startsWith('#')) continue;
    for (const [pattern, what] of REFUSED) {
      if (pattern.test(line)) {
        throw new CorpusManifestError(
          `${path}:${index + 1} uses ${what}, which this scanner refuses to guess at`,
        );
      }
    }
    // Anything indented belongs to a nested collection, and no path is read out of one.
    if (/^ {2,}/.test(line)) continue;

    const element = /^- (.*)$/.exec(line);
    if (element && sequence) {
      sequence.push(unquote(element[1]!));
      continue;
    }

    const entry = /^(\w+):\s*(.*)$/.exec(line);
    if (!entry) {
      throw new CorpusManifestError(`${path}:${index + 1} is not a key, an item or a continuation`);
    }
    if (entry[2] === '') {
      // A key with an empty value opens a sequence — which is also what a key with a nested mapping
      // under it leaves behind, and an empty one of those names no paths either way.
      sequence = [];
      out[entry[1]!] = sequence;
    } else {
      sequence = null;
      out[entry[1]!] = unquote(entry[2]!);
    }
  }

  return out;
}

/** A sequence of paths — `vertices` and `edges` on the index, and nothing else. */
export function paths(manifest: ScannedManifest, key: string): string[] {
  const value = manifest[key];
  return Array.isArray(value) ? value.map(String) : [];
}

/** Join dataset-relative segments the way the manifest writes them: forward slashes, always. */
export function join(...parts: Array<string | undefined>): string {
  return parts
    .filter((part): part is string => part !== undefined && part !== '')
    .map((part, index) => (index === 0 ? part.replace(/\/+$/, '') : part.replace(/^\/+|\/+$/g, '')))
    .filter((part) => part !== '')
    .join('/');
}
