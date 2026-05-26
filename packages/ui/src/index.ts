/**
 * @fossil-lang/ui — public entry. Radix primitives + Fossil design tokens.
 * Per ADR-0033 + ADR-0034 + Phase 10 plans 10-01 (scaffold) +
 * 10-03/04/05 (primitives).
 */
export * from './primitives/index.js';
export { cx } from './utils/cx.js';
export type { ClassValue } from './utils/cx.js';
export { injectFossilUiStyles } from './styles/inject.js';
