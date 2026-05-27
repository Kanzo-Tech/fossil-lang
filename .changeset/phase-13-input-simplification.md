---
'@fossil-lang/playground': minor
'@fossil-lang/wasm': minor
---

Phase 13 — input-model simplification (ADR-0037): drop user-facing CSVW; infer types from runtime files.

The user no longer authors a CSVW JSON-LD descriptor. The playground orchestrates host-side DuckDB-WASM `DESCRIBE` introspection BEFORE every compile and hands the result to the Rust compiler via a new WASM API; the native CLI (`fossil-cli`) mirrors the same flow via the `duckdb` crate. Restores the v0.1 product promise "10 seconds to a triple": write `io.csv("path")` and the compiler does the rest.

## `@fossil-lang/playground` (minor)

- **CSVW descriptor panel REMOVED.** `packages/playground/src/csvw/` deleted (5 files: `CsvwPreview.tsx`, `apply.ts`, `infer.ts`, `index.ts`, `type-map.ts`; ~480 LOC). Users author `.fossil` and the compiler infers schema automatically.
- **New `useInferredDescriptors` hook** (`packages/playground/src/hooks/useInferredDescriptors.ts`): extracts `io.csv("...")` / `io.json("...")` source bindings via regex, resolves URLs via the existing `ConnectionResolver`, runs DuckDB-WASM `DESCRIBE`, and calls `FossilPlayground.registerInferredDescriptor(...)`. Same regex shape + DuckDB-type-to-primitive table as the fossil-cli sibling — single source of truth for both consumers.
- **Default landing example switched** from `helloExample.mapping` (CSVW-sidecar) to `helloNoCsvwExample.mapping` (canonical v0.2 style — `io.csv("...")` without `schema=`).
- **Permalink schema bumped v1 → v2** (`PermalinkStateV2`). The v1→v2 migration silently DROPS the `csvw` field; encoder never emits `csvw`. v0.1 bookmarks still load.
- **Bundle decrease**: `@fossil-lang/playground` dist JS dropped ~57 KB (from 230.98 KB at Phase 12 baseline to 173.93 KB) — primarily the deleted `csvw/` directory.

## `@fossil-lang/wasm` (minor)

- **New API**: `FossilPlayground.registerInferredDescriptor(descriptor: InferredDescriptorJson)` — pre-compile registration of host-introspected column lists. JSON-stringifies the descriptor internally; throws on malformed payload.
- **New types** exported from `@fossil-lang/wasm`: `InferredDescriptorJson`, `InferredColumnJson`, `InferredPrimitive` (string-literal union of `Integer | Float | String | Bool | Date | DateTime | Time | GYear | AnyURI`).
- **Rust-side** (transitive — not separately published): `fossil-descriptors-input` adds `InferredDescriptor` + `InferredColumn` concrete structs (Salsa-friendly, Send + Sync + Clone + Hash + Eq + serde); `fossil-base::System` trait gains `inferred_descriptor` / `register_inferred_descriptor` accessors; `fossil-hir` forward-prop tries the inferred path FIRST, falls back to legacy CSVW with `D-CSVW-DEPRECATED` warning.

## Backwards compatibility

- **v0.1 `.fossil` files with explicit `schema = "..."` arg**: still parse + compile correctly. The checker emits `D-CSVW-DEPRECATED` warning (severity = warning; does NOT fail compilation). Hard removal of the grammar production deferred to v0.3 / v1.0 per ADR-0037.
- **v0.1 permalinks**: load successfully via the v1→v2 migration. The `csvw` field is silently discarded.
- **WASM compile API surface unchanged**: `compile()` / `compile_file()` / `check()` / `diagnostics_for()` signatures stable. `registerInferredDescriptor` is purely additive.
- **`@fossil-lang/editor`, `@fossil-lang/viewer`, `@fossil-lang/ui`, `@kanzo/theme`**: unaffected. CSVW lived only in `@fossil-lang/playground` + Rust crates.

## ADR + traceability

- **ADR-0037** — Drop user-facing CSVW; infer input schema via host-side DuckDB DESCRIBE; introduce InferredDescriptor. Partially supersedes ADR-0007 (user-facing surface only; intermediate CSVW IR + thin parser retained).
- **Requirements discharged**: INPUT-01, INPUT-02, INPUT-03 (all Complete).
- **Plans**: 13-01 (ADR + type), 13-02 (HIR rewire), 13-03 (WASM API), 13-04a (CLI pre-introspection), 13-04b (playground refactor + CSVW deletion), 13-05 (permalink v2 + landing default), 13-06 (phase close).

Walking-skeleton invariant preserved across the transition: `fossil compile examples/hello.fossil` produces byte-identical 283 B manifest + 784 B parquet (Phase 12 baseline) BOTH with the legacy CSVW sidecar (when present) AND without (inferred path).
