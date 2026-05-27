/**
 * Phase 13 migration coverage (ADR-0037).
 *
 * The v0.1 permalink schema (`v: 1`) carried an optional `csvw` field. v0.2
 * (`v: 2`) drops user-facing CSVW — the v1→v2 migration registered in
 * envelope.ts SILENTLY DROPS the csvw field, preserves source + shex,
 * bumps the version stamp.
 *
 * These tests exercise the migration explicitly via hand-built v1 envelopes
 * (so we don't depend on the committed fixture file's specific shape).
 * Paper-permanence claim: old bookmarks still load, just without CSVW —
 * which is fine because v0.2 infers schema from runtime files.
 */

import { describe, test, expect } from 'vitest';
import { gzipSync, gunzipSync, strToU8, strFromU8 } from 'fflate';
import {
  encode as encodePermalink,
  decode as decodePermalink,
  SCHEMA_VERSION as PERMALINK_SCHEMA_VERSION,
} from '../src/permalink/index.js';
import { toB64Url, fromB64Url } from '../src/permalink/envelope.js';

/**
 * Build a v1-shaped permalink byte-for-byte from a raw payload (= what an
 * older playground built and put in a 2026 bookmark URL). Same wire format
 * (gzip + base64url) as the current encoder; the difference is the JSON
 * envelope shape and the `v: 1` stamp.
 */
function buildV01Permalink(payload: {
  source: string;
  csvw?: string;
  shex?: string;
}): string {
  const envelope = {
    v: 1 as const,
    source: payload.source,
    ...(payload.csvw !== undefined ? { csvw: payload.csvw } : {}),
    ...(payload.shex !== undefined ? { shex: payload.shex } : {}),
  };
  const json = JSON.stringify(envelope);
  const gz = gzipSync(strToU8(json), { level: 9 });
  return toB64Url(gz);
}

describe('Phase 13 permalink v0.1 → v0.2 migration (ADR-0037)', () => {
  test('SCHEMA_VERSION advanced to 2', () => {
    expect(PERMALINK_SCHEMA_VERSION).toBe(2);
  });

  test('decode of v1 payload WITH csvw silently drops the csvw field', () => {
    const v01 = buildV01Permalink({
      source:
        'prefix ex: <https://example.org/>\nusers := io.csv("@examples/hello.csv")\n',
      csvw:
        '{"@context":"http://www.w3.org/ns/csvw","tableSchema":{"columns":[{"name":"id","datatype":"integer"}]}}',
      shex: 'prefix ex: <https://example.org/>\nex:Person {}',
    });
    const state = decodePermalink(v01);
    // The v1→v2 migration must have fired — stamp is now 2.
    expect(state.v).toBe(2);
    // source + shex preserved verbatim
    expect(state.source).toContain('io.csv("@examples/hello.csv")');
    expect(state.shex).toContain('prefix ex:');
    // csvw silently dropped
    expect((state as Record<string, unknown>).csvw).toBeUndefined();
  });

  test('decode of v1 payload WITHOUT csvw still works (no spurious csvw)', () => {
    const v01 = buildV01Permalink({
      source: 'users := io.csv("u.csv")\n',
    });
    const state = decodePermalink(v01);
    expect(state.v).toBe(2);
    expect(state.source).toBe('users := io.csv("u.csv")\n');
    expect(state.shex).toBeUndefined();
    expect((state as Record<string, unknown>).csvw).toBeUndefined();
  });

  test('v2 encoder NEVER emits a csvw field', () => {
    // Encode a fresh v2 state and decode the raw envelope to inspect keys.
    const encoded = encodePermalink({
      source: 'fresh v2 source\n',
      shex: 'ex:T {}',
    });
    // Reverse the wire format manually so we can introspect the JSON.
    const gz = fromB64Url(encoded);
    const json = strFromU8(gunzipSync(gz));
    const obj = JSON.parse(json) as Record<string, unknown>;
    expect(obj.v).toBe(2);
    expect(obj.source).toBe('fresh v2 source\n');
    expect(obj.shex).toBe('ex:T {}');
    expect('csvw' in obj).toBe(false);
  });

  test('round-trip: encode(decode(v01 with csvw)) loses csvw but keeps source + shex', () => {
    const v01 = buildV01Permalink({
      source: 'roundtrip-with-csvw',
      csvw: '{}',
      shex: 'shex-text',
    });
    const decoded = decodePermalink(v01);
    const reencoded = encodePermalink({
      source: decoded.source,
      ...(decoded.shex !== undefined ? { shex: decoded.shex } : {}),
    });
    const redecoded = decodePermalink(reencoded);
    expect(redecoded.v).toBe(2);
    expect(redecoded.source).toBe('roundtrip-with-csvw');
    expect(redecoded.shex).toBe('shex-text');
    expect((redecoded as Record<string, unknown>).csvw).toBeUndefined();
  });

  test('malformed payload still errors gracefully', () => {
    // Random non-base64url garbage — decoder should throw a descriptive Error.
    expect(() => decodePermalink('!!!not-a-permalink!!!')).toThrow();
  });
});
