/**
 * Barrel for the Turtle serializer module.
 *
 * Public API (re-exported from `@fossil-lang/playground`):
 *   - rowsToTurtle(vertices, edges, prefixes) — synchronous TTL writer
 *   - VertexRow, EdgeRow — row shapes used by the writer
 *   - TurtleTab — React tab panel rendering the serialized Turtle (PLAY-10).
 *
 * Foundation for PLAY-10.
 */

export { rowsToTurtle } from './render.js';
export type { VertexRow, EdgeRow } from './render.js';
export { TurtleTab, type TurtleTabProps } from './TurtleTab.js';
