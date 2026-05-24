/**
 * useResetPlayground — the ADR-0026 enforcement mechanism.
 *
 * Per ADR-0026 (Two Workers, Two Lifecycles):
 *   - DuckDB Worker terminates+recreates on Reset (reclaims the ~6.4 MB heap)
 *   - LSP Worker stays warm (editor session keeps its models, undo stack, etc.)
 *
 * The asymmetric API IS the enforcement:
 *   - `resetDuckDb` is imported and called
 *   - `resetLsp` is INTENTIONALLY ABSENT — no such symbol exists anywhere in
 *     this package's surface
 *
 * Adding `resetLsp()` here would require a deliberate code change that a
 * reviewer would pause over (per ADR-0026 § Consequences "auditable in code").
 *
 * The CONN-01 invariant test grepps the source tree to confirm no `resetLsp`
 * export ever leaks out; future ADRs that supersede 0026 will need to
 * explicitly amend both this file AND the test's allowlist.
 */

import { useCallback } from 'react';
import { resetDuckDb } from './useDuckDb.js';

/**
 * Reset the playground per ADR-0026: terminate the DuckDB Worker; LEAVE
 * the LSP Worker untouched. The editor session (editor models, cursor
 * position, undo stack, LSP cold-start) stays warm; only the DuckDB heap
 * is reclaimed.
 *
 * DO NOT add `resetLsp()` here without a deliberate ADR superseding 0026.
 * The asymmetric API IS the enforcement mechanism.
 */
export function useResetPlayground(): () => void {
  return useCallback(() => {
    resetDuckDb();
    // INTENTIONAL: no LSP reset. See ADR-0026.
  }, []);
}
