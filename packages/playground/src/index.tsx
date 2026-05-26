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

// usePermalink hook — PLAY-04 wiring (decodes initialPermalink on mount +
// emits debounced onStateChange on edits). Advanced consumers can call this
// hook directly when composing a custom layout outside <FossilPlayground/>
// (the multi-host fixture pattern from 08-09 SUMMARY).
export { usePermalink } from './hooks/usePermalink.js';
export type { UsePermalinkArgs } from './hooks/usePermalink.js';

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

// Run pipeline — the orchestration helper handleRun composes. Exposed so
// advanced consumers building custom layouts (the multi-host fixture in
// 08-11 is the canonical example) can reuse the same four-step pipeline
// without re-implementing compile → resolver.resolve → COPY-rewrite →
// DuckDB execute. Per 08-13 Task 1 + 08-09 SUMMARY advanced-composition path.
export { runPipeline } from './run/runPipeline.js';
export type {
  RunPipelineDeps,
  RunPipelineInput,
  RunPipelineResult,
} from './run/runPipeline.js';
export {
  extractSourceRefs,
  rewriteCopyToCreateTable,
  transformSql,
} from './run/transformSql.js';
export type { TableClass, TransformedSql } from './run/transformSql.js';

// Permalink (PLAY-04) — stateless gzip+base64url URL-fragment encoder/decoder.
// Aliased re-exports so library consumers can call encodePermalink /
// decodePermalink without colliding with any encoding helpers they may
// already have in scope. The PermalinkStateV1 type is exposed for callers
// who want to round-trip the shape through their own state machine
// (e.g. apps/landing wiring window.location.hash <-> playground state).
export {
  encode as encodePermalink,
  decode as decodePermalink,
  SCHEMA_VERSION as PERMALINK_SCHEMA_VERSION,
  MAX_PERMALINK_BYTES,
  PermalinkTooLargeError,
} from './permalink/index.js';
export type { PermalinkStateV1 } from './permalink/index.js';

// Turtle serializer (PLAY-10) — synchronous TTL writer wrapping n3.Writer.
// Aliased types (TurtleVertexRow / TurtleEdgeRow) avoid collision with the
// VertexRow / EdgeRow already exported from FossilPlayground's prop surface.
export { rowsToTurtle } from './turtle/index.js';
export type {
  VertexRow as TurtleVertexRow,
  EdgeRow as TurtleEdgeRow,
} from './turtle/index.js';

// Turtle tab (PLAY-10) — React tab panel rendering the serialized Turtle
// next to Graph + Edges in the result panel. Pure presentational; takes
// pre-adapted TurtleVertexRow / TurtleEdgeRow arrays (the FossilPlayground
// component owns the adapter from its native VertexRow / EdgeRow shape).
export { TurtleTab } from './turtle/index.js';
export type { TurtleTabProps } from './turtle/index.js';

// Compiled SQL panel (PLAY-07) — read-only CodeMirror 6 view with the
// DuckDB dialect of @codemirror/lang-sql. Subscribes to the parent's `sql`
// prop; the SQL string is sourced from FossilPlayground::compileFile() in
// the integrated component, and can be sourced from any caller's compile
// path when consuming CompiledSqlPanel standalone (advanced composition).
export { CompiledSqlPanel, DuckDB } from './compiled-sql/index.js';
export type { CompiledSqlPanelProps } from './compiled-sql/index.js';

// CSVW inference (PLAY-09 + PLAY-11) — DuckDB DESCRIBE → minimal CSVW JSON-LD.
// The pure type-map (duckdbTypeToCsvw) is exported for callers that already
// have a DESCRIBE result in hand and only need the type translation.
// `applyCsvw` / `parseCsvw` are the JSON round-trip helpers consumed by the
// editable preview AND by advanced consumers that want to validate persisted
// descriptors before round-tripping through their own state machine.
// `CsvwPreview` is the editable React panel (PLAY-09) that mounts inside the
// playground component but is also exported standalone for advanced layouts.
export {
  inferCsvw,
  duckdbTypeToCsvw,
  applyCsvw,
  parseCsvw,
  CsvwPreview,
} from './csvw/index.js';
export type { CsvwTable, CsvwColumn, CsvwPreviewProps } from './csvw/index.js';

// BibTeX modal (PLAY-08) — cite modal triggered from the playground toolbar.
// Embeds the current permalink URL so the cite round-trips state; pre-bundled
// with the Min Oo & Hartig (ESWC 2025) foundational-paper reference per
// 09-CONTEXT.md. The cite-templates helpers are exported separately so
// advanced consumers (the landing hero is the canonical example) can render
// the BibTeX inline without mounting the modal.
export { BibtexModal } from './bibtex/index.js';
export type { BibtexModalProps } from './bibtex/index.js';
export {
  HARTIG_BIBTEX,
  HARTIG_PLAINTEXT,
  buildPlaygroundBibtex,
  buildPlaygroundPlaintext,
} from './bibtex/index.js';

// ============================================================================
// Re-export @fossil-lang/ui — primitives + tokens + utilities
//
// v0.2 zero-code-change upgrade path for the primitive surface: consumers
// who only import from '@fossil-lang/playground' get the Radix primitives
// (Tabs, Dialog, ...) without adding a new package to their dependencies.
// Per ADR-0033 + Phase 10 CONTEXT.md.
//
// Direct consumers can still `import { Tabs } from '@fossil-lang/ui'` if they
// want the smaller dependency footprint. Both paths resolve to the same
// module instance (pnpm workspace deduplication).
//
// Name-collision audit (2026-05-26): @fossil-lang/playground exports
// lightTheme, darkTheme, themeToCssVars, cssVarsToStyle, useTheme,
// FossilPlayground (+ sub-components and hooks). @fossil-lang/ui exports cx,
// injectFossilUiStyles, and the 8 Radix primitives + their sub-components
// (Tabs/TabsList/.../Toggle/ToggleGroup/.../ResizablePanelGroup/...). No
// overlap.
//
// NOTE: brand-owned theme values (kanzoTheme + <KanzoThemeProvider/>) live
// in the SEPARATE @kanzo/theme package — NOT re-exported here. Per ADR-0035
// (visual ownership separation, plan 10-09): @fossil-lang/* is brand-agnostic;
// kanzo-branded hosts install @kanzo/theme directly. The brief 10-06
// `fossilIdeTheme` named export was removed in plan 10-09.
// ============================================================================
export * from '@fossil-lang/ui';
