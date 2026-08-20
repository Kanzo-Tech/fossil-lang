/**
 * The GraphAr manifest, scanned the way a stranger reads it.
 *
 * A corpus cannot be discovered by listing a directory — over HTTP there is no listing — so it has
 * one entry point, `graph.graph.yml`, and everything else is reached from there. This module turns
 * the manifest set a host has already fetched into the fields {@link resolveCorpus} addresses with.
 *
 * **A line scanner and not a YAML parser, deliberately**, and this is the third copy of that
 * decision in the tree rather than a fourth way of doing it. `apps/corpus/guards/manifest.mjs`
 * makes the same call and cannot be imported — it has no `package.json` and exists to be *copied*
 * by a third party who has neither this repository nor npm. `fossil-graph`'s Rust side deserialises
 * through `serde_yaml_ng`, behind the WASM module this package exists to not require. What the
 * manifest uses is a flat mapping of scalars, one sequence of paths and one sequence of small
 * mappings, and that is the grammar below.
 *
 * **What it refuses rather than guesses:** anchors and aliases, flow style (`[a, b]`), multi-line
 * scalars (`|`, `>`), and any nesting deeper than one sequence of mappings. A manifest using them
 * throws with the line named, which is the honest outcome — a scanner that silently reads half a
 * document composes half the URLs and reports none of it.
 */

/** Dataset-relative location of the aggregate index. The one path a reader is told. */
export const GRAPH_INFO_PATH = 'graph.graph.yml';

/** One scanned manifest file: scalars, one sequence of strings, one sequence of mappings. */
export type ScannedManifest = Record<string, string | string[] | Array<Record<string, string>>>;

/** Raised by everything in this module. Carries the file it was reading. */
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
  let sequence: string[] | Array<Record<string, string>> | null = null;
  let item: Record<string, string> | null = null;

  const lines = text.split('\n');
  for (const [index, raw] of lines.entries()) {
    const line = raw.replace(/\s+$/, '');
    if (line === '' || line.trimStart().startsWith('#')) continue;
    for (const [pattern, what] of REFUSED) {
      if (pattern.test(line)) {
        throw new CorpusManifestError(
          `${path}:${index + 1} uses ${what}, which this scanner refuses to guess at`,
        );
      }
    }

    // A continuation of the mapping currently being built inside a sequence.
    const continuation = /^ {2}(\w+):\s*(.*)$/.exec(line);
    if (continuation && item) {
      item[continuation[1]!] = unquote(continuation[2]!);
      continue;
    }
    // Anything else indented belongs to a nested collection — a property list inside a property
    // group — and no address is composed from one. Skipped rather than refused: it is legal, it is
    // just not addressed by any convention, and refusing it would fail on a corpus this reads fine.
    if (/^ {2,}/.test(line)) continue;

    const element = /^- (.*)$/.exec(line);
    if (element && sequence) {
      const pair = /^(\w+):\s*(.*)$/.exec(element[1]!);
      if (pair) {
        item = { [pair[1]!]: unquote(pair[2]!) };
        (sequence as Array<Record<string, string>>).push(item);
      } else {
        item = null;
        (sequence as string[]).push(unquote(element[1]!));
      }
      continue;
    }

    const entry = /^(\w+):\s*(.*)$/.exec(line);
    if (!entry) {
      throw new CorpusManifestError(
        `${path}:${index + 1} is not a key, an item or a continuation`,
      );
    }
    item = null;
    if (entry[2] === '') {
      sequence = [];
      out[entry[1]!] = sequence;
    } else {
      sequence = null;
      out[entry[1]!] = unquote(entry[2]!);
    }
  }

  return out;
}

/** A scalar the address depends on. Absent is an error, because a default would compose a URL. */
export function required(manifest: ScannedManifest, path: string, key: string): string {
  const value = manifest[key];
  if (typeof value !== 'string' || value === '') {
    throw new CorpusManifestError(`${path} declares no ${key}, so it addresses nothing`);
  }
  return value;
}

/** A scalar that has to be a positive integer — a chunk size, and nothing else so far. */
export function requiredNumber(manifest: ScannedManifest, path: string, key: string): number {
  const raw = required(manifest, path, key);
  const value = Number(raw);
  if (!Number.isInteger(value) || value <= 0) {
    throw new CorpusManifestError(`${path} declares ${key}: ${raw}, which is not a row count`);
  }
  return value;
}

/** A sequence of paths — `vertices` and `edges` on the index. */
export function paths(manifest: ScannedManifest, key: string): string[] {
  const value = manifest[key];
  if (!Array.isArray(value)) return [];
  return value.map(String);
}

/** A sequence of small mappings — `adj_lists`, and nothing else so far. */
export function mappings(manifest: ScannedManifest, key: string): Array<Record<string, string>> {
  const value = manifest[key];
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is Record<string, string> => typeof entry === 'object');
}

/** Join dataset-relative segments the way the manifest writes them: forward slashes, always. */
export function join(...parts: Array<string | undefined>): string {
  return parts
    .filter((part): part is string => part !== undefined && part !== '')
    .map((part, index) => (index === 0 ? part.replace(/\/+$/, '') : part.replace(/^\/+|\/+$/g, '')))
    .filter((part) => part !== '')
    .join('/');
}
