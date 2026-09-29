/**
 * The SQL the reader writes, at the one place a value becomes text: literals, identifiers, and the
 * guards that turn what an engine hands back into a 64-bit id or a coordinate.
 */

import type { QueryRow } from './query.js';

/**
 * Raised when the bytes disagree with what the manifest promised, or when the corpus is shaped in a
 * way this API cannot answer. Distinct from {@link CorpusManifestError}, which is the manifest
 * failing to address itself before a single byte of payload has been read — a caller can retry one
 * of those against a different corpus and never the other.
 */
export class CorpusReadError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CorpusReadError';
  }
}

/** A single-quoted SQL string literal. Every URL the reader composes reaches SQL through here. */
export function lit(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

/** A DuckDB list of paths, which is how one query spans a set of tiles. */
export function list(urls: readonly string[]): string {
  return `[${urls.map(lit).join(', ')}]`;
}

/**
 * Distinct, in order — what a set of tiles is a set of FILES.
 *
 * A no-op under the file-per-tile container and load-bearing under the other, where every tile of a
 * type names one file: `read_parquet` over a list scans each element, so naming it once per tile
 * would return every row once per tile.
 */
export function distinct(urls: readonly string[]): string[] {
  return [...new Set(urls)];
}

/** A quoted SQL identifier. Column names come from the payload, so they are not interpolated raw. */
export function ident(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}

export const ascending = (a: bigint, b: bigint): number => (a < b ? -1 : a > b ? 1 : 0);

/**
 * A 64-bit id out of an untyped column.
 *
 * The corpus contract's second obligation is that a 64-bit id is a `BigInt` at the TypeScript
 * boundary, and it records exactly this hole: *"an id read out of a Parquet column into an untyped
 * value is outside it."* Hosts disagree about what they hand back — DuckDB-WASM gives a
 * `UINTEGER` as a `Number` and a `UBIGINT` as a `BigInt`, and a host that goes through JSON turns
 * both into numbers — so this is where the width is enforced instead of assumed. A `Number` that is not
 * a safe integer is refused rather than rounded, because rounding a `dense_id` addresses a
 * different vertex and nothing downstream can tell.
 */
export function idOf(value: unknown, what: string): bigint {
  if (typeof value === 'bigint') return value;
  if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) return BigInt(value);
  if (typeof value === 'string' && /^\d+$/.test(value)) return BigInt(value);
  throw new CorpusReadError(
    `${what} came back as ${typeof value} ${String(value)}, which cannot hold a 64-bit id exactly`,
  );
}

export function floatOf(value: unknown, what: string): number {
  const n = typeof value === 'string' ? Number(value) : value;
  if (typeof n !== 'number' || !Number.isFinite(n)) {
    throw new CorpusReadError(`${what} came back as ${String(value)}, which is not a coordinate`);
  }
  return n;
}

export function text(row: QueryRow, column: string): string {
  const value = row[column];
  if (typeof value !== 'string') {
    throw new CorpusReadError(`expected ${column} to be text; got ${typeof value}`);
  }
  return value;
}
