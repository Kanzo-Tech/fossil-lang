/**
 * Permalink roundtrip spec (PLAY-04).
 *
 * Asserts encode → decode is the identity over a representative set of
 * state shapes: empty, minimal hello, source+csvw+shex, unicode/escapes,
 * and a 10 KB source. Also verifies the encoded output is pure base64url
 * (no `+`, `/`, `=`) — required for safe URL-fragment storage without
 * additional percent-encoding.
 *
 * Why a table-driven `for…of` instead of `test.each`: the existing tests
 * in this package use the same per-case `test(name, …)` pattern (see
 * transformSql.test.ts) — staying consistent keeps Vitest output uniform
 * across the suite.
 */

import { describe, test, expect } from 'vitest';
import {
  encodePermalink,
  decodePermalink,
  PERMALINK_SCHEMA_VERSION,
  type PermalinkStateV1,
} from '../src/index.js';

describe('permalink roundtrip (PLAY-04)', () => {
  const cases: Array<{ name: string; state: Omit<PermalinkStateV1, 'v'> }> = [
    { name: 'empty source', state: { source: '' } },
    {
      name: 'minimal hello',
      state: { source: 'prefix ex: <https://example.org/>\n' },
    },
    {
      name: 'source + csvw + shex',
      state: {
        source:
          'prefix ex: <https://example.org/>\nusers := io.csv("@examples/hello.csv")\n',
        csvw:
          '{"@context":"http://www.w3.org/ns/csvw","url":"@examples/hello.csv"}',
        shex: 'prefix ex: <https://example.org/>\nex:Person {}',
      },
    },
    {
      name: 'unicode + escapes',
      state: { source: 'prefix ε: <Δ>\n"line\\nbreak"\n€\n' },
    },
    { name: 'large source (10 KB)', state: { source: 'x'.repeat(10_000) } },
  ];

  for (const { name, state } of cases) {
    test(name, () => {
      const encoded = encodePermalink(state);
      const decoded = decodePermalink(encoded);
      expect(decoded.v).toBe(PERMALINK_SCHEMA_VERSION);
      expect(decoded.source).toBe(state.source);
      expect(decoded.csvw).toBe(state.csvw);
      expect(decoded.shex).toBe(state.shex);
    });
  }

  test('encoded string contains only base64url chars (no `+`, `/`, `=`)', () => {
    const encoded = encodePermalink({ source: 'hello world' });
    expect(encoded).toMatch(/^[A-Za-z0-9_-]+$/);
  });

  test('encode is deterministic — same input yields same output', () => {
    // gzip with a fixed level is content-deterministic across fflate calls.
    // We rely on this for stable URL fragments (refreshing a tab shouldn't
    // produce a "new" link). If fflate ever introduces non-determinism we
    // will catch it here.
    const state = { source: 'prefix ex: <https://example.org/>\n' };
    const a = encodePermalink(state);
    const b = encodePermalink(state);
    expect(a).toBe(b);
  });

  test('round-trip is idempotent — encode(decode(encode(x))) === encode(x)', () => {
    const state = {
      source: 'prefix ex: <https://example.org/>\nusers := io.csv("@x.csv")\n',
      csvw: '{}',
    };
    const once = encodePermalink(state);
    const decoded = decodePermalink(once);
    // Re-encode from the decoded shape (omitting `v` because encode injects it).
    const twice = encodePermalink({
      source: decoded.source,
      ...(decoded.csvw !== undefined ? { csvw: decoded.csvw } : {}),
      ...(decoded.shex !== undefined ? { shex: decoded.shex } : {}),
    });
    expect(twice).toBe(once);
  });
});
