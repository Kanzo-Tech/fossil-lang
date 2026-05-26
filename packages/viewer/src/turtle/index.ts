/**
 * Barrel for the Turtle serializer module.
 *
 * Moved from `packages/playground/src/turtle/` to `@fossil-lang/viewer`
 * in Phase 12 plan 12-04. The aliasing of the row types
 * (VertexRow → TurtleVertexRow, EdgeRow → TurtleEdgeRow) lives here so
 * downstream consumers re-export by a one-liner instead of carrying the
 * aliasing themselves.
 *
 * Public API:
 *   - rowsToTurtle(vertices, edges, prefixes) — synchronous TTL writer.
 *   - TurtleVertexRow, TurtleEdgeRow — row shapes used by the writer.
 *   - TurtleTab — React tab panel rendering the serialized Turtle.
 */

export { rowsToTurtle } from './render.js';
export type {
  VertexRow as TurtleVertexRow,
  EdgeRow as TurtleEdgeRow,
} from './render.js';
export { TurtleTab, type TurtleTabProps } from './TurtleTab.js';
