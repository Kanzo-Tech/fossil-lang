/**
 * @fossil-lang/playground — public entry point.
 *
 * Top-level React component + sub-components + hooks per the package's
 * advanced-composition contract (ADR-0028). Consumers typically import the
 * default `<FossilPlayground/>` only; advanced consumers can mix the
 * sub-components into their own layouts.
 *
 * NOTE the deliberate absence of `resetLsp` — per ADR-0026, the asymmetric
 * API IS the enforcement mechanism. See `useResetPlayground.ts` for the
 * audit-trail comment.
 */

// Top-level component
export { FossilPlayground } from './component/FossilPlayground.js';
export type {
  FossilPlaygroundProps,
  VertexRow,
  EdgeRow,
} from './component/FossilPlayground.js';

// Sub-components — advanced composition
export { FossilEditor } from './component/FossilEditor.js';
export type { FossilEditorProps } from './component/FossilEditor.js';
export { ResultTable } from './component/ResultTable.js';
export type { ResultTableProps } from './component/ResultTable.js';
export { ResultGraph } from './component/ResultGraph.js';
export type { ResultGraphProps } from './component/ResultGraph.js';

// Hooks
export { useLspWorker } from './hooks/useLspWorker.js';
export type { UseLspWorkerOpts } from './hooks/useLspWorker.js';
export { useDuckDb, getDuckDb, resetDuckDb } from './hooks/useDuckDb.js';
export { useResetPlayground } from './hooks/useResetPlayground.js';
export { useTheme } from './hooks/useTheme.js';
export type { UseThemeResult } from './hooks/useTheme.js';

// Theme — built-in light + dark + the flat-CSS-var helpers (THEME-01)
export { lightTheme } from './theme/light.js';
export { darkTheme } from './theme/dark.js';
export {
  themeToCssVars,
  cssVarsToStyle,
  CSS_VAR_PREFIX,
} from './theme/tokens.js';

// Accessibility primitives (A11Y-01)
export { announce, ARIA_LABELS, LIVE_REGION_ID } from './a11y/index.js';

// Transport adapter
export { createWorkerTransport } from './lsp/WorkerTransport.js';
