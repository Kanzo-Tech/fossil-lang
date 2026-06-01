/**
 * Source-binding introspection helpers — now a thin re-export of the canonical
 * `@fossil-lang/introspect` package.
 *
 * `extractSourceRefs` + `duckdbTypeToFossilPrimitive` previously lived inlined
 * here (Phase 14 plan 14-02). They are the SAME logic the fossil-cli Rust
 * sibling + keasy need, so they were lifted into `@fossil-lang/introspect` (the
 * one home; see .planning/EDITOR-SCHEMA-AWARE-PLAN.md). This shim keeps the
 * `run/` barrel + the existing test imports working byte-for-byte while the
 * implementation lives in one place.
 */

export {
  extractSourceRefs,
  duckdbTypeToFossilPrimitive,
} from '@fossil-lang/introspect';
