/**
 * Permalink decoder (PLAY-04). base64url → ungzip (fflate) → JSON → migrate.
 *
 * Symmetric to encode.ts. The migration step is what makes a 2026 v1 fixture
 * still decodable in 2027 if/when v2 ships — see envelope.ts `migrate`.
 *
 * On malformed input (bad base64, bad gzip, bad JSON, missing `v`) the decoder
 * throws a descriptive Error. The future PLAY-04 host (plan 09-05) will catch
 * these and render an "invalid permalink — please regenerate" message rather
 * than crashing the playground.
 */

import { gunzipSync, strFromU8 } from 'fflate';
import { fromB64Url, migrate } from './envelope.js';
import type { PermalinkStateV1 } from './envelope.js';

export function decode(s: string): PermalinkStateV1 {
  // Defend against the common "user pasted a URL including `#`" case so the
  // error message points at the right call site upstream.
  if (typeof s !== 'string' || s.length === 0) {
    throw new Error('Permalink decode: input must be a non-empty string');
  }

  let gz: Uint8Array;
  try {
    gz = fromB64Url(s);
  } catch (cause) {
    throw new Error(
      `Permalink decode: invalid base64url — ${(cause as Error).message}`,
      { cause },
    );
  }

  let json: string;
  try {
    json = strFromU8(gunzipSync(gz));
  } catch (cause) {
    throw new Error(
      `Permalink decode: gzip ungzip failed — ${(cause as Error).message}`,
      { cause },
    );
  }

  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch (cause) {
    throw new Error(
      `Permalink decode: invalid JSON — ${(cause as Error).message}`,
      { cause },
    );
  }

  if (
    typeof raw !== 'object' ||
    raw === null ||
    typeof (raw as { v?: unknown }).v !== 'number'
  ) {
    throw new Error('Permalink decode: invalid envelope — missing numeric `v` field');
  }

  return migrate(raw as { v: number } & Record<string, unknown>);
}
