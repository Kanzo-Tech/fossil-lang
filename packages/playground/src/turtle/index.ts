/**
 * v0.1.x backwards-compat shim — the canonical turtle module now lives in
 * `@fossil-lang/viewer` (Phase 12 plan 12-04). This file re-exports the
 * same symbols so consumers using
 * `import { rowsToTurtle } from '@fossil-lang/playground'` keep working
 * without code changes.
 *
 * Module-instance dedup via pnpm workspace symlinks (same pattern Phase
 * 11 shipped for `@fossil-lang/editor`).
 *
 * The aliased re-export (`TurtleVertexRow as VertexRow`) preserves the
 * EXACT v0.1.x export name — internal playground callers using
 * `import { VertexRow } from './turtle/index.js'` continue to compile.
 */

export { rowsToTurtle, TurtleTab } from '@fossil-lang/viewer';
export type {
  TurtleVertexRow as VertexRow,
  TurtleEdgeRow as EdgeRow,
  TurtleTabProps,
} from '@fossil-lang/viewer';
