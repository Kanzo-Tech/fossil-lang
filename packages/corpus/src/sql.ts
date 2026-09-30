/** The one place a value becomes SQL text: a literal, or an identifier. */

/**
 * Raised when a read the manifest allowed fails, or when a call names what the manifest does not
 * declare — a table, a column. Distinct from `CorpusManifestError`, which is the manifest itself.
 */
export class CorpusReadError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CorpusReadError';
  }
}

/** A single-quoted SQL string literal. Every path the reader composes reaches SQL through here. */
export function lit(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

/** A quoted SQL identifier. Names come from the manifest, so they are never interpolated raw. */
export function ident(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}
