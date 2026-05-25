/**
 * Permalink encoder (PLAY-04). JSON → gzip (fflate) → base64url.
 *
 * Why JSON-then-gzip-then-base64url:
 *   - JSON gives schema-agnostic structure; the `v` field is the first key so
 *     parsers can fail fast on version mismatch.
 *   - gzip @ level 9 compresses .fossil source 60-80% (mostly keyword
 *     repetition); the cost is ~3ms for a 10 KB input — invisible to the user.
 *   - base64url is URL-fragment-safe (no `+`, `/`, `=` to percent-encode);
 *     this lets the host page paste the encoded string straight into
 *     `location.hash` without further escaping.
 *
 * The encoder always injects `v: SCHEMA_VERSION` regardless of what the caller
 * passed — this prevents a stray `v: 99` from contaminating a v1 fixture and
 * makes the round-trip stable.
 */

import { gzipSync, strToU8 } from 'fflate';
import {
  toB64Url,
  MAX_PERMALINK_BYTES,
  SCHEMA_VERSION,
} from './envelope.js';
import type { PermalinkStateV1 } from './envelope.js';

export { MAX_PERMALINK_BYTES };

/**
 * Thrown when the encoded permalink exceeds MAX_PERMALINK_BYTES.
 *
 * Callers (the future PLAY-04 UI in plan 09-05) catch this to render a
 * "permalink too large — consider sharing via a Gist" affordance per the
 * 09-CONTEXT.md decision: ≥10 KB suggests Gist (deferred).
 */
export class PermalinkTooLargeError extends Error {
  override readonly name = 'PermalinkTooLargeError';
  constructor(public sizeBytes: number) {
    super(
      `Permalink exceeds ${MAX_PERMALINK_BYTES} bytes (${sizeBytes}). Consider sharing via Gist instead.`,
    );
  }
}

/**
 * Encode playground state into a URL-fragment-safe permalink string.
 *
 * The input type omits `v` because the encoder injects it deterministically.
 * Accepting a partial type prevents callers from accidentally pinning to an
 * old SCHEMA_VERSION at the call site.
 */
export function encode(state: Omit<PermalinkStateV1, 'v'>): string {
  // Always inject `v: SCHEMA_VERSION` first so the JSON key order is stable
  // and the version field appears at the head of the envelope. JSON.stringify
  // honors insertion order for non-integer keys (per ES2015+).
  const envelope: PermalinkStateV1 = {
    v: SCHEMA_VERSION,
    source: state.source,
    ...(state.csvw !== undefined ? { csvw: state.csvw } : {}),
    ...(state.shex !== undefined ? { shex: state.shex } : {}),
  };
  const json = JSON.stringify(envelope);
  const gz = gzipSync(strToU8(json), { level: 9 });
  const b64 = toB64Url(gz);
  if (b64.length > MAX_PERMALINK_BYTES) {
    throw new PermalinkTooLargeError(b64.length);
  }
  return b64;
}
