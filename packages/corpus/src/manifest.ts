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

/**
 * One scanned manifest file: scalars, sequences of strings, sequences of mappings, and mappings.
 *
 * The last of those arrived late and the way its absence hid is worth keeping: a key with an empty
 * value was always read as opening a SEQUENCE, so `index:` scanned to `[]` — truthy, carrying no
 * `prefix` — and every reader concluded the corpus declares no index. **A manifest that says
 * something the scanner cannot see reads exactly like one that does not say it.**
 */
export type ScannedManifest = Record<
  string,
  string | string[] | Array<Record<string, string>> | Record<string, string | string[]>
>;

/** A nested mapping's values: scalars, and the one sequence of scalars a mapping carries. */
export type ScannedMapping = Record<string, string | string[]>;

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
  /** The nested MAPPING being continued — `index:`, `codes:`, `levels:`. Never a sequence element. */
  let map: ScannedMapping | null = null;
  /** A key whose value was empty and whose shape the next child line decides. */
  let pending: string | null = null;
  /**
   * The last key ON {@link item} whose value was empty — the one a `- ` line under it would be an
   * element of. `levels:` inside `levels:` is the case: a mapping carrying a sequence, which is one
   * level deeper than anything this grammar had, and the level list is the whole of what the
   * pyramid declares that a reader cannot derive.
   */
  let nested: string | null = null;


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
    if (continuation && map) {
      map[continuation[1]!] = unquote(continuation[2]!);
      nested = continuation[2] === '' ? continuation[1]! : null;
      continue;
    }
    if (continuation && item) {
      item[continuation[1]!] = unquote(continuation[2]!);
      continue;
    }
    // An element of a sequence nested in a mapping, and ONLY when it is a scalar. serde writes such
    // items at their key's own indentation, so this is the same two spaces a continuation has.
    //
    // **A `- k: v` item is left exactly where it was**, which is skipped and the key still `''`.
    // That is not timidity: `properties:` inside a `property_groups` element is a sequence of
    // MAPPINGS, this reads no column list off it (`openCorpus` reads the bytes instead, and says
    // why), and turning those items into mangled scalars would be a scanner inventing a shape
    // rather than growing one.
    const element2 = /^ {2}- (.*)$/.exec(line);
    if (element2 && map && nested !== null && !/^\w+:/.test(element2[1]!)) {
      const held = map[nested];
      const held2 = Array.isArray(held) ? held : [];
      held2.push(unquote(element2[1]!));
      map[nested] = held2;
      continue;
    }
    // The first child of a key with an empty value, and it is `k: v` rather than `- `: the key is
    // a MAP. Decided here rather than at the key, because `property_groups:` and `index:` are
    // written identically until this line arrives.
    if (continuation && pending !== null) {
      const built: ScannedMapping = { [continuation[1]!]: unquote(continuation[2]!) };
      out[pending] = built;
      sequence = null;
      item = null;
      map = built;
      nested = continuation[2] === '' ? continuation[1]! : null;
      pending = null;
      continue;
    }
    // Anything else indented belongs to a nested collection — a property list inside a property
    // group — and no address is composed from one. Skipped rather than refused: it is legal, it is
    // just not addressed by any convention, and refusing it would fail on a corpus this reads fine.
    if (/^ {2,}/.test(line)) continue;

    const element = /^- (.*)$/.exec(line);
    if (element && pending !== null) {
      sequence = [];
      out[pending] = sequence;
      pending = null;
    }
    if (element && sequence) {
      map = null;
      nested = null;
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
    map = null;
    nested = null;
    if (entry[2] === '') {
      // Shape unknown until the first child line. A key with an empty value and no children stays
      // a sequence, which is what `property_groups: []` has always been.
      sequence = [];
      out[entry[1]!] = sequence;
      pending = entry[1]!;
    } else {
      sequence = null;
      pending = null;
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

/**
 * A declared row count — `vertex_count` or `edge_count` — as a `BigInt`, or `null` when absent.
 *
 * `null` rather than `0`, because the two are different findings and only one of them is a corpus:
 * a declared `0` is an empty type and legal, an absent count is a manifest that cannot say how far
 * the corpus goes. {@link resolveCorpus} composes URLs either way; {@link openCorpus} refuses,
 * because enumerating tiles is the one thing the count is for.
 *
 * **Parsed to `BigInt` off the digits, never through `Number`.** `requiredNumber` beside this would
 * be wrong for exactly the case `apps/corpus/guards/vectors.json` publishes: at 2⁵³+1 a `Number`
 * count reads as 2⁵³, the ceiling comes out one tile short, and the tail tile disappears from a
 * reader that never asks for it. Anything that is not a run of decimal digits is `null` — a scanner
 * that guessed would be deciding what the writer meant.
 */
export function optionalCount(manifest: ScannedManifest, key: string): bigint | null {
  const value = manifest[key];
  if (typeof value !== 'string' || !/^\d+$/.test(value.trim())) return null;
  return BigInt(value.trim());
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

/**
 * A nested mapping under `key`, or `null` when the manifest has none.
 *
 * `null` covers both "the key is absent" and "the key is an empty collection", because those are
 * the same fact to a reader: nothing to compose an address from. A key that is a SEQUENCE is not
 * `null` and not a mapping either — that is a manifest saying something this cannot read, and it
 * throws rather than degrading to "declares none", which is exactly how the index went invisible
 * before the scanner could see a nested map.
 */
export function mapping(
  manifest: ScannedManifest,
  path: string,
  key: string,
): ScannedMapping | null {
  const value = manifest[key];
  if (value === undefined) return null;
  if (Array.isArray(value)) {
    if (value.length === 0) return null;
    throw new CorpusManifestError(`${path} writes ${key} as a list, and it is a mapping here`);
  }
  if (typeof value === 'string') {
    throw new CorpusManifestError(`${path} writes ${key} as a scalar, and it is a mapping here`);
  }
  return value;
}

/**
 * A scalar inside a nested mapping, or `undefined` — the accessor that keeps a caller from having
 * to narrow {@link ScannedMapping}'s union at every read.
 *
 * A key whose value is a SEQUENCE reads as absent rather than throwing, because the two callers
 * that ask this are asking *did the writer say this*, and a list where a scalar belongs is a
 * manifest neither of them can act on.
 */
export function scalar(map: ScannedMapping, key: string): string | undefined {
  const value = map[key];
  return typeof value === 'string' ? value : undefined;
}

/**
 * A sequence of scalars inside a nested mapping — `levels:`'s own `levels:`, and nothing else yet.
 *
 * Empty covers absent, a scalar, and an empty list, on {@link mapping}'s argument: all three are
 * *the writer declared no levels*, and a pyramid that is declared without its numbers is one a
 * reader cannot address. The caller that cares says so with its own error.
 */
export function list(map: ScannedMapping, key: string): string[] {
  const value = map[key];
  return Array.isArray(value) ? value : [];
}
