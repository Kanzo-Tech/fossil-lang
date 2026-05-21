# ADR 0018: Drive ShEx-driven sink decomposition from a descriptor argument (option b)

**Date:** 2026-05-21
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/05-stdlib-sources-graphar-sink-complete/05-RESEARCH.md` §"ShEx-Driven Decomposition (SC#4 — the load-bearing one)"; Phase 3 deferred item #3 (`Db::system()` → `output_descriptor_kind()` wiring); `decisions/0006-output-descriptor-kind-enum-dispatch.md`; `decisions/rudof-wasm.md` (Pitfall 1).

## Context

SC#4 requires the GraphAr sink to perform **ShEx-driven vertex/edge decomposition** and produce real Parquet files: a target shape with N shapes decomposes into N vertex tables + M inter-shape edge tables, with duplicate subjects merged per ShEx cardinality and the IRI used verbatim as the vertex id.

The shape information the decomposition needs is the Phase-3 `ShExDescriptor` (`ShapeBinding` / `ResolvedConstraint` / `Cardinality`), which is already ref-resolved and cardinality-decoded. The open question (raised in Phase 3, deferred to here) is **how the descriptor reaches the sink**:

- **(a)** Wire `Db::system()` to expose `output_descriptor_kind()`, resolve the target shape inside the type-checker path (`typecheck_mapping` / `lower_to_mir`), and thread it through to codegen. This is the "complete" end-to-end story but requires `Db`-trait surgery and risks widening a per-mapping Salsa query (`MAX_PER_MAPPING_FAN_OUT = 1`; Pitfall 6).
- **(b)** Pass the resolved `OutputDescriptorKind` (carrying the `ShExDescriptor`) **directly as a function argument** into the sink's decomposition entry point, bypassing `Db::system()`.
- **(c)** Re-parse the ShEx schema inside the sink (rejected: duplicates Phase-3 work, re-introduces the OneOf/cycle handling).

A second force: `fossil-sinks` is the 7th WASM-gated crate (ADR-0017). Adding `fossil-descriptors-output` as a dependency must not break the WASM gate (`shex_ast` could in principle pull a non-WASM dep). The Phase 0 rudof spike (`decisions/rudof-wasm.md`) and the Phase 3 re-verification established that `shex_ast` is WASM-clean **as long as only `Schema::from_reader` is used** (never `Schema::from_iri`, which pulls `reqwest`/`tokio`); `ShExDescriptor::from_reader` honors this.

## Decision

We will drive the sink decomposition from a **descriptor argument (option b)**. The entry point is:

```rust
pub fn vertex_edge_decomp<'db>(
    plan: &MirGraph<'db>,
    db: &'db dyn fossil_base::Db,
    kind: &OutputDescriptorKind,   // <-- passed in, NOT read via Db::system()
    chunk_size: u64,
) -> SinkPlan
```

with a `db`-free core `vertex_edge_decomp_from_kind(kind, source_relation, chunk_size)` shared by the fixture/unit-test path. The decomposition is **plain Rust over the resolved structs** — no `Box<dyn Trait>`, no Salsa read of the descriptor, fan-out-safe.

The full `Db`-wiring (option a) stays **deferred to Phase 6**. When Phase 6 lands it, the *same* `vertex_edge_decomp` is fed the `OutputDescriptorKind` resolved from `db.system()` instead of from a host/test argument — **zero decomposition-code changes**. The seam is identical; Phase 6 simply changes who supplies the `kind`.

**WASM cfg-fence mechanism:** **none required.** `fossil-descriptors-output` compiles cleanly to `wasm32-unknown-unknown` (verified: `cargo check --target wasm32-unknown-unknown -p fossil-descriptors-output` succeeds; `cargo tree -i mio -p fossil-sinks --target wasm32-unknown-unknown` returns zero paths after the dependency is added). The decomposition-consuming code therefore needs **no** `#[cfg(not(target_arch = "wasm32"))]` fence — `fossil-sinks` stays in the 7-crate WASM gate as-is. **Plan 05-08 inherits this:** no cfg-fence propagation through `fossil-codegen` is needed for the descriptor path; the only native-only boundary remains the runtime COPY execution (ADR-0017), unchanged.

## Consequences

**Positive.**
- Minimal scope, real files: option (b) gives the sink the shape table it needs to split vertices/edges and merge cardinality with no `Db` surgery and no fan-out risk. SC#4 produces 2 vertex + 1 edge decomposition (and, in 05-08, real Parquet) via a fixture-constructed `ShExDescriptor`.
- Reuses everything Phase 3 built (`ShExDescriptor`/`ShapeBinding`/`ResolvedConstraint`/`Cardinality`) — already ref-resolved + cardinality-decoded.
- Phase 6 lights it up for free: the descriptor source changes from a host argument to `db.system()`; the decomposition is untouched.
- The WASM gate stays green with no conditional compilation — the manifest + decomposition layer remains identical native and WASM, so the playground can show the decomposition SQL in-browser (PLAY-07).

**Negative / honest discharge mode.**
- With option (b), SC#4's decomposition is driven by a **host/test-supplied ShEx fixture**, not by a `.fossil`-declared `from <shape>` resolved end-to-end through the type-checker. The honest discharge label is **"sink-path real files via a fixture ShEx"**, explicitly *not* "end-to-end from source through the type-checker." This mirrors Phase 3's helper-proven labeling and respects the STATE.md "Do NOT fake" rule: the decomposition is real and exercised; the *input wiring* (descriptor-from-source) is the Phase-6 piece.

**Neutral.**
- The `db` parameter on `vertex_edge_decomp` is used only to read `plan.ops(db)` for the source-relation derivation (a placeholder in v0.1, the real relation subquery in 05-08). Keeping it in the signature now means the Phase-6 wiring needs no signature change.
- Adding `fossil-descriptors-output` + `shex_ast` + `prefixmap` to `fossil-sinks` widens its dependency graph, but all are WASM-clean and introduce no new `cargo deny` advisory or ban beyond the pre-existing inherited ones.
