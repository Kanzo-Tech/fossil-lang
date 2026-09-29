import type { CorpusAddressing } from '../src/address.js';
import { open } from '../src/corpus.js';

/**
 * The addressing of a corpus opened over manifests already in hand — `open(base, { query,
 * manifestFiles })` and then `.addressing`, the way a viewer reads its URLs.
 *
 * The engine answers every statement with no rows: `DESCRIBE` finds no columns, so no type has
 * geometry, no footer is read, and nothing past the manifests is asked of it. What comes back is
 * the arithmetic over the manifests and nothing an engine could have added.
 */
export async function addressingOf(
  base: string,
  manifestFiles: Record<string, string>,
): Promise<CorpusAddressing> {
  return (await open(base, { query: async () => [], manifestFiles })).addressing;
}
