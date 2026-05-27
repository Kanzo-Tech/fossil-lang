/**
 * `component/` barrel — internal-only.
 *
 * Re-exports the top-level `<FossilPlayground/>` plus the IDE-tabs panel
 * sub-components (Mapping / Source / Shape / Output) introduced in Phase 14
 * plan 14-03 + the legacy `ResultTable` / `ResultGraph` (the latter is a
 * v0.2.x deprecated alias of `<FossilGraphView/>` from `@fossil-lang/viewer`).
 *
 * The package's outermost barrel (`src/index.tsx`) cherry-picks the v0.1.x
 * public exports from here; the panel sub-components stay INTERNAL to the
 * playground composition (advanced consumers compose from `@fossil-lang/editor`
 * + `@fossil-lang/viewer` directly).
 */

export { FossilPlayground } from './FossilPlayground.js';
export type {
  FossilPlaygroundProps,
  VertexRow,
  EdgeRow,
} from './FossilPlayground.js';

export { MappingPanel } from './MappingPanel.js';
export type { MappingPanelProps } from './MappingPanel.js';
export { SourcePanel } from './SourcePanel.js';
export type {
  SourcePanelProps,
  SourceColumn,
  SourceSchema,
} from './SourcePanel.js';
export { ShapePanel } from './ShapePanel.js';
export type { ShapePanelProps } from './ShapePanel.js';
export { OutputPanel } from './OutputPanel.js';
export type { OutputPanelProps } from './OutputPanel.js';

export { ResultTable } from './ResultTable.js';
export type { ResultTableProps } from './ResultTable.js';
export { ResultGraph } from './ResultGraph.js';
export type { ResultGraphProps } from './ResultGraph.js';
