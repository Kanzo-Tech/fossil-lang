/**
 * `run/` barrel — consolidated public surface of the playground's Run
 * orchestration layer.
 *
 * Per CONTEXT.md COMP-02: `run/ — runPipeline + transformSql + introspection`.
 *
 * Internal-only — the package's outermost barrel (`src/index.tsx`) cherry-picks
 * the v0.1.x public exports (runPipeline + transformSql helpers) from the
 * sub-modules directly to keep the public surface frozen for backwards
 * compatibility. The introspection helpers stay internal (consumed only by
 * `useInferredDescriptors` + the run/ tests).
 */

export { runPipeline } from './runPipeline.js';
export type {
  RunPipelineDeps,
  RunPipelineInput,
  RunPipelineResult,
} from './runPipeline.js';

export {
  extractSourceRefs,
  rewriteCopyToCreateTable,
  transformSql,
} from './transformSql.js';
export type { TableClass, TransformedSql } from './transformSql.js';

// Phase 13 InferredDescriptor introspection helpers (plan 14-02). Re-exported
// here under a disambiguating alias because `transformSql.ts` also exports
// `extractSourceRefs` (a different regex variant for COPY-rewriting). The
// `extractInferredSourceRefs` name keeps the run/ barrel collision-free
// while leaving the function's local name unchanged for the hook + tests.
export {
  extractSourceRefs as extractInferredSourceRefs,
  duckdbTypeToFossilPrimitive,
} from './introspection.js';
