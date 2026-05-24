/**
 * ADR-0026 enforcement tests.
 *
 * The asymmetric API IS the enforcement mechanism: this file structurally
 * asserts that:
 *   1. resetDuckDb is exported + callable + idempotent
 *   2. resetLsp is NOWHERE on the package surface (no export from
 *      useResetPlayground; no export from the package index)
 *   3. useResetPlayground's source file does not declare a `resetLsp`
 *      symbol (the docstring mentions its absence, but no code declares it)
 *
 * If any of these flips — e.g. a future contributor adds an `export function
 * resetLsp()` somewhere — the test fails LOUDLY. Per ADR-0026 § Consequences
 * "the asymmetric API makes the decision auditable in code".
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { resetDuckDb } from '../src/hooks/useDuckDb.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

describe('ADR-0026: Reset terminates DuckDB Worker but NOT LSP Worker', () => {
  it('resetDuckDb is callable + idempotent on a never-loaded DuckDB', () => {
    expect(() => resetDuckDb()).not.toThrow();
    expect(() => resetDuckDb()).not.toThrow();
  });

  it('useResetPlayground module exports NO resetLsp symbol', async () => {
    const module = await import('../src/hooks/useResetPlayground.js');
    expect((module as Record<string, unknown>).resetLsp).toBeUndefined();
  });

  it('package index exports NO resetLsp symbol', async () => {
    const allExports = await import('../src/index.js');
    expect((allExports as Record<string, unknown>).resetLsp).toBeUndefined();
  });

  it('useResetPlayground source declares no resetLsp symbol (only docstring mentions)', () => {
    const sourcePath = resolve(__dirname, '../src/hooks/useResetPlayground.ts');
    const source = readFileSync(sourcePath, 'utf8');
    // Match declarations: `export function resetLsp`, `function resetLsp`,
    // `const resetLsp = `, `let resetLsp = `, `var resetLsp = `,
    // `export const resetLsp`, `export let resetLsp`, `export var resetLsp`.
    const declarationPattern =
      /(?:^|\n)\s*(?:export\s+)?(?:async\s+)?(?:function|const|let|var)\s+resetLsp\b/;
    expect(declarationPattern.test(source)).toBe(false);
    // Sanity: docstring DOES mention resetLsp (otherwise the audit trail is missing).
    expect(source).toMatch(/resetLsp/);
  });
});
