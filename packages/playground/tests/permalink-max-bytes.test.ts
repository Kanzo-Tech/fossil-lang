/**
 * Permalink size-budget spec (PLAY-04).
 *
 * Three guards:
 *   1. The canonical `hello` example (the one the playground ships with —
 *      the "10-second example" CTA on the landing page) must fit under
 *      MAX_PERMALINK_BYTES = 8000. If a future grammar change bloats this
 *      below-budget hello, the test fails and the planner reconsiders
 *      either the grammar or the cap.
 *   2. A deliberately oversize, gzip-resistant source MUST throw
 *      PermalinkTooLargeError. We use a high-entropy ASCII span (printable
 *      chars 33..123) so gzip can't trivially collapse it.
 *   3. A doubling probe finds the encoder's effective ceiling. This is
 *      the safety floor for the "caller relies on encode never returning
 *      a >MAX string" contract; the UI in plan 09-05 will assume that.
 *
 * Why not assert an exact byte count for hello: gzip output is determined
 * by zlib heuristics that can shift with library version. We assert the
 * upper bound only and let the SUMMARY record today's actual size as a
 * baseline for future drift detection.
 */

import { describe, test, expect } from 'vitest';
import {
  encodePermalink,
  MAX_PERMALINK_BYTES,
  PermalinkTooLargeError,
} from '../src/index.js';

/**
 * High-entropy filler — a linear-congruential generator gives ASCII output
 * with no periodicity that gzip can exploit. A cyclic `i % 90` pattern (the
 * obvious naive choice) gets crushed by gzip's repeat detector to a tiny
 * constant — 50 KB → ~330 bytes — and never trips the size cap. The LCG
 * here matches Numerical Recipes' "quick and dirty" constants (1664525,
 * 1013904223); good enough to defeat DEFLATE's window matcher, which is all
 * we need for a size-cap test.
 */
function lcgFiller(n: number, seed = 42): string {
  let s = seed >>> 0;
  let out = '';
  for (let i = 0; i < n; i++) {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    out += String.fromCharCode(33 + (s % 90));
  }
  return out;
}

describe('permalink max-bytes budget (PLAY-04)', () => {
  test('canonical hello example fits under MAX_PERMALINK_BYTES', () => {
    const hello = `prefix ex: <https://example.org/>
users := io.csv("@examples/hello.csv")
User : ex:Person from users
    iri = \`\${ex:}user/\${.id}\`
    ex:name = .name
`;
    const encoded = encodePermalink({ source: hello });
    expect(encoded.length).toBeLessThan(MAX_PERMALINK_BYTES);
    expect(encoded.length).toBeGreaterThan(0);
  });

  test('deliberately oversize high-entropy source throws PermalinkTooLargeError', () => {
    // 20_000 LCG-random chars — empirically encodes to ~22 KB base64url
    // (far above the 8 KB cap). See the doubling probe below for the
    // empirical compression ratio.
    const huge = lcgFiller(20_000);
    expect(() => encodePermalink({ source: huge })).toThrowError(
      PermalinkTooLargeError,
    );
  });

  test('encoder never returns a string >= MAX_PERMALINK_BYTES (doubling probe)', () => {
    // Probe doubling input size until we either find the threshold (encoder
    // throws) or exhaust the range. The strict-less-than assertion is the
    // safety contract callers rely on: `encode` returns a string of length
    // < MAX, or throws.
    let i = 1_000;
    while (i < 64_000) {
      try {
        const encoded = encodePermalink({ source: lcgFiller(i) });
        expect(encoded.length).toBeLessThan(MAX_PERMALINK_BYTES);
        i *= 2;
      } catch (e) {
        expect(e).toBeInstanceOf(PermalinkTooLargeError);
        // Sanity: the error carries the actual size and it must be > MAX.
        expect((e as PermalinkTooLargeError).sizeBytes).toBeGreaterThan(
          MAX_PERMALINK_BYTES,
        );
        return;
      }
    }
    throw new Error(
      'Encoder never produced a too-large permalink in the probed range — entropy filler likely too compressible',
    );
  });

  test('PermalinkTooLargeError carries the offending size', () => {
    const huge = lcgFiller(20_000);
    try {
      encodePermalink({ source: huge });
      throw new Error('expected throw — encode should have raised PermalinkTooLargeError');
    } catch (e) {
      expect(e).toBeInstanceOf(PermalinkTooLargeError);
      expect((e as PermalinkTooLargeError).sizeBytes).toBeGreaterThan(
        MAX_PERMALINK_BYTES,
      );
      expect((e as PermalinkTooLargeError).name).toBe('PermalinkTooLargeError');
    }
  });
});
