import { addressManifests, type CorpusAddressing } from '../src/address.js';

/**
 * The addressing of a corpus over manifests already in hand — what `open` resolves before it reads a
 * byte, and nothing an engine could have added. Async so a refusal reads as a rejection, the way
 * `open` delivers it.
 */
export async function addressingOf(
  base: string,
  manifestFiles: Record<string, string>,
): Promise<CorpusAddressing> {
  return addressManifests(manifestFiles, base);
}
