/**
 * apply.ts — round-trip helpers between the in-memory `CsvwTable` shape and
 * the JSON-string form that the playground's CSVW state hook persists.
 *
 * Why a separate module (not inline in `CsvwPreview.tsx`):
 *   - The serializer is the single home for future CSVW JSON-LD canonicalisation
 *     (datatype URI normalisation, `@base` defaulting, prefix expansion).
 *   - The parser is also called from `FossilPlayground.tsx` (when the user
 *     hits the Reset button — we need to validate the persisted JSON before
 *     handing it back to the preview component).
 *   - Pure, dep-free, side-effect-free — keeps the unit tests trivial.
 *
 * Per 09-07-PLAN Task 1 (PLAY-09 + PLAY-11).
 */

import type { CsvwTable } from './infer.js';

/**
 * Serialise a `CsvwTable` to a stable, diff-friendly JSON string.
 *
 * `JSON.stringify(table, null, 2)` is deterministic for the `CsvwTable` shape
 * (object key order = insertion order on modern engines; our writers always
 * insert `@context` → `url` → `tableSchema` in that order). Round-trip with
 * `parseCsvw` is therefore byte-identity preserving for any unmodified table.
 */
export function applyCsvw(table: CsvwTable): string {
  return JSON.stringify(table, null, 2);
}

/**
 * Parse a CSVW JSON string back into the in-memory `CsvwTable` shape.
 *
 * Validates the minimal-descriptor invariants:
 *   - parses as JSON (returns `null` on syntax error — never throws);
 *   - `@context` MUST be the literal CSVW namespace string;
 *   - `tableSchema.columns` MUST be an array.
 *
 * Anything else returns `null` so the caller can fall back to the inferred
 * value (or refuse to overwrite the user's edit). The strictness here is
 * deliberate — a soft-failing parser would let a typo in the descriptor
 * silently corrupt the compile.
 */
export function parseCsvw(json: string): CsvwTable | null {
  try {
    const obj = JSON.parse(json) as unknown;
    if (typeof obj !== 'object' || obj === null) return null;
    const rec = obj as Record<string, unknown>;
    if (rec['@context'] !== 'http://www.w3.org/ns/csvw') return null;
    const schema = rec.tableSchema as { columns?: unknown } | undefined;
    if (!schema || !Array.isArray(schema.columns)) return null;
    return obj as CsvwTable;
  } catch {
    return null;
  }
}
