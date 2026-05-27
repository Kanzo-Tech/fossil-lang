/**
 * Permalink forward-compat spec (PLAY-04 SC#1).
 *
 * The committed fixtures in `./fixtures/permalink-fixtures.json` are real
 * base64url-encoded permalinks generated on 2026-05-25 (the schema-v1 era).
 * Any future PR that regresses the encoder/decoder — bumps SCHEMA_VERSION
 * without adding a migration, swaps fflate for a non-byte-compatible
 * library, changes the JSON key order, etc. — must keep these fixtures
 * decoding to their expected state, or it must add a migration so they
 * still arrive at the current SCHEMA_VERSION.
 *
 * Critically: NEVER edit existing fixture entries. Always APPEND new ones.
 * Editing breaks the paper-permanence guarantee — an ESWC/ISWC reviewer
 * clicks the 2026 URL in 2028 and expects the same mapping back.
 *
 * The shape of `fixtures[]` is deliberately schema-free JSON (no Zod, no
 * codec) so the file remains human-auditable. The TypeScript-side fixture
 * shape is asserted ad-hoc below; mismatches surface as runtime errors,
 * which is fine for a test fixture.
 */

import { describe, test, expect } from 'vitest';
import {
  decodePermalink,
  PERMALINK_SCHEMA_VERSION,
} from '../src/index.js';
import fixturesFile from './fixtures/permalink-fixtures.json';

type Fixture = {
  note: string;
  encodedAt: string;
  encoded: string;
  expectedSource: string;
  expectedCsvw?: string;
  expectedShex?: string;
};

describe('permalink forward-compat (PLAY-04 SC#1)', () => {
  const fixtures = fixturesFile.fixtures as Fixture[];

  // Sanity — we should never accidentally ship an empty fixture array; that
  // would silently disable the entire forward-compat guard.
  test('fixture file has at least one entry', () => {
    expect(fixtures.length).toBeGreaterThan(0);
  });

  for (const fx of fixtures) {
    test(`fixture encoded ${fx.encodedAt} (${fx.note}) still decodes`, () => {
      const state = decodePermalink(fx.encoded);

      // Migration must always land us on the current SCHEMA_VERSION (even
      // if the fixture itself was a lower version — migrate() forwards it).
      expect(state.v).toBe(PERMALINK_SCHEMA_VERSION);

      // Payload fields must match the fixture's expected values exactly.
      expect(state.source).toBe(fx.expectedSource);

      // Phase 13 v0.2 (ADR-0037): the v2 schema dropped user-facing CSVW.
      // The v1→v2 migration silently DROPS any `csvw` field. v0.1 fixtures
      // that had `expectedCsvw` now decode WITHOUT csvw — that's the
      // load-bearing migration claim. We deliberately do NOT update the
      // fixture file (paper-permanence — old fixtures are immutable); we
      // just adjust the assertion to expect the migration's drop behaviour.
      expect((state as Record<string, unknown>).csvw).toBeUndefined();

      if (fx.expectedShex !== undefined) {
        expect(state.shex).toBe(fx.expectedShex);
      } else {
        expect(state.shex).toBeUndefined();
      }
    });
  }
});
