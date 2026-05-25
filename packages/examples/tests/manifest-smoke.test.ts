/**
 * PLAY-05 manifest-smoke harness — Phase 9 plan 09-09 Task 3.
 *
 * Asserts that `packages/examples/src/manifest.json` is in sync with the
 * filesystem reality (every declared file resolves on disk) and structurally
 * sound (≥6 examples, unique ids, each example carries a variations/ subdir).
 *
 * Why a separate test from variations.test.ts: the variations harness spawns
 * `fossil check` per file (slow, needs the cargo build). This smoke is pure
 * fs + JSON inspection — runs in <50ms and fails fast on manifest drift
 * BEFORE the CI runner spends 2 minutes building cargo to discover that the
 * manifest references a missing file. Cheap pre-check, expensive post-check.
 */
import { describe, test, expect } from 'vitest';
import manifest from '../src/manifest.json';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

const EXAMPLES_DIR = resolve(__dirname, '../src');

describe('manifest smoke (PLAY-05)', () => {
  test('manifest has ≥ 6 examples (PLAY-05 SC#2)', () => {
    expect(manifest.examples.length).toBeGreaterThanOrEqual(6);
  });

  test('every manifested file path resolves on disk', () => {
    // Each entry's `files.*` is an `@examples/<filename>` connector path.
    // The on-disk location is `{EXAMPLES_DIR}/{example.id}/{filename}`.
    for (const ex of manifest.examples) {
      for (const [kind, rel] of Object.entries(ex.files)) {
        const filename = String(rel).replace(/^@examples\//, '');
        const fsPath = join(EXAMPLES_DIR, ex.id, filename);
        expect(
          existsSync(fsPath),
          `${ex.id}.${kind} → ${fsPath} missing on disk`,
        ).toBe(true);
      }
    }
  });

  test('every example has a variations/ directory', () => {
    for (const ex of manifest.examples) {
      const varDir = join(EXAMPLES_DIR, ex.id, 'variations');
      expect(
        existsSync(varDir),
        `${ex.id}: variations/ subdir missing — required by PLAY-05`,
      ).toBe(true);
    }
  });

  test('each example id is unique', () => {
    const ids = manifest.examples.map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  test('every example carries at least a fossil + csv file (descriptors optional)', () => {
    for (const ex of manifest.examples) {
      expect(ex.files.fossil, `${ex.id}: missing 'fossil' file entry`).toBeDefined();
      // At least one CSV-shaped file (`csv`, `csv2`, …) — JSON inputs would
      // use a `json` key in a future RML2.0 example.
      const hasCsvOrJson = Object.entries(ex.files).some(
        ([k]) => k.startsWith('csv') || k === 'json',
      );
      expect(hasCsvOrJson, `${ex.id}: needs at least one csv|json source`).toBe(
        true,
      );
    }
  });
});
