# ADR 0020: Thread the output descriptor through a fossil-hir-owned `HirDb` extension accessor (R2)

**Date:** 2026-05-21
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/06-cli-complete-lsp/06-RESEARCH.md` §Db-Wiring (R1 vs R2) + Open Questions #2 (R2 cycle-safety) and #3 (lsp-types wasm32); Phase 3 `deferred-items.md` #3 + #8; `decisions/0006-output-descriptor-kind-enum-dispatch.md`; `decisions/0018-shex-decomposition-wiring.md` (option-b "argument, not key" seam); `decisions/rudof-wasm.md` (Path (b) AcceptAll fallback).

## Context

Phase 3 left `fossil_hir::shapes::resolve_target_shape` returning `None`
unconditionally. The cause: `fossil_base::Db::system()` returns
`&dyn fossil_base::System`, which does NOT carry the
`fossil_descriptors_output::SystemWithDescriptors` extension vtable. ADR-0006
(Option B) keeps `fossil-base` descriptor-ignorant on purpose — the descriptor
type catalogue lives entirely in `fossil-descriptors-output`, so a thin `Db`
trait object cannot reach a host's `OutputDescriptorKind`. The consequence
(recorded in Phase 3 `deferred-items.md` #3 + #8) was that a real host `ShEx`
schema never flowed into `typecheck_mapping`: backward `ShEx` checking (SC#2)
and OneOf surfacing (SC#4) were only *helper-proven*, and SC#4 target-side
hover and SC#5 split-mapping (Phase 6) were blocked end-to-end.

Two wiring shapes were on the table (Research §Db-Wiring):

- **R1** — put `output_descriptor_kind()` directly on `fossil_base::Db`. This
  would force `fossil-base` to depend on `fossil-descriptors-output`, inverting
  the natural dependency direction (`fossil-descriptors-output -> fossil-base`)
  and violating ADR-0006. Research Open Question #2 flagged this as a potential
  crate cycle; the Wave-0 spike below confirmed R1 is the wrong shape.
- **R2** — put the accessor on a `fossil-hir`-OWNED extension trait
  (`HirDb: fossil_base::Db`). `fossil-hir` already depends on
  `fossil-descriptors-output`, so naming `OutputDescriptorKind` there adds no
  new crate edge and no cycle.

A hard constraint throughout: the descriptor must NOT widen any
`#[salsa::tracked]` query's key (`MAX_PER_MAPPING_FAN_OUT = 1`, Phase 2 plan
02-07 invariant), must never be interned, and must never be wrapped in
`Box<dyn Trait>` (CLAUDE.md hard rule + ADR-0006).

## Decision

We will wire the descriptor via **R2**: a `fossil-hir`-owned extension trait

```rust
pub trait HirDb: fossil_base::Db {
    fn output_descriptor_kind(&self) -> &OutputDescriptorKind {
        &OutputDescriptorKind::ACCEPT_ALL_DEFAULT  // degraded fallback
    }
}
```

and `resolve_target_shape` consumes the descriptor as a **plain borrowed
argument**, exactly the ADR-0018 "descriptor as argument, not key" seam:

```rust
pub fn resolve_target_shape<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
    kind: &OutputDescriptorKind,   // plain value, read once; never a Salsa key
) -> Option<ResolvedShape<'db>>
```

`ShEx(d)` looks up the mapping's fully-resolved shape IRI
(`d.lookup_shape(&iri)`) and returns `Some(ResolvedShape)`; `AcceptAll(_)`
returns `None`. The descriptor never enters a tracked query's key, is never
interned, and is never a trait object — so the per-mapping fan-out is
unchanged (the 10-mapping invalidation fixture supplies `AcceptAll`, still
resolves to `None`, and `invalidation_regression` stays green).

The tracked `typecheck_mapping(db, mapping)` (which only has the thin
`&dyn fossil_base::Db`, never `HirDb`) supplies `ACCEPT_ALL_DEFAULT` in-query —
keeping the query's key descriptor-free and the fixture's behaviour identical.
A host that has loaded a `ShEx` schema drives the `Some` path by calling
`resolve_target_shape` with the value it pulls from
`HirDb::output_descriptor_kind()` on its own concrete `Db`. `fossil-base` stays
descriptor-ignorant; `R1` is explicitly NOT pursued.

### Wave-0 spike outcomes

- **Spike A (lsp-types wasm32, Research Open Question #3): GREEN.** `lsp-types`
  0.97 compiles to `wasm32-unknown-unknown` (`cargo check --target
  wasm32-unknown-unknown -p fossil-ide` succeeds with the dep added). Therefore
  `fossil-ide` MAY return `lsp_types` structs directly (the recommended
  downstream API shape) rather than Fossil-domain byte-range intermediates
  translated in `fossil-lsp`. The dep stays in `fossil-ide`.
- **Spike B (R2 cycle-safety, Research Open Question #2): cycle-free.** The
  `HirDb` trait lives in `fossil-hir`, which already depends on
  `fossil-descriptors-output`; `cargo check -p fossil-hir` and the
  `wasm32-unknown-unknown` gate both pass, confirming no new crate edge and no
  cycle. R1 (accessor on `fossil-base::Db`) was not attempted — it would
  invert the dependency direction per ADR-0006.

## Consequences

**Positive.**
- `resolve_target_shape` now returns `Some(ResolvedShape)` in production when
  the host supplies a `ShEx` `OutputDescriptorKind` declaring the mapping's
  target shape — the Phase-3 deferral (#3 + #8) is resolved. SC#4 target-side
  hover and SC#5 split-mapping become reachable end-to-end.
- Fan-out-safe: the descriptor is a plain argument, never a Salsa key, never
  interned, no `Box<dyn>`. `MAX_PER_MAPPING_FAN_OUT` stays `1`; the invalidation
  regression is untouched.
- `fossil-base` stays descriptor-ignorant (ADR-0006 preserved); the accessor is
  isolated in `fossil-hir`. No crate cycle (Spike B).
- The `AcceptAll` degraded fallback (rudof-wasm.md Path (b)) is retained as the
  `HirDb` default and the in-query value, so the walking-skeleton and the
  10-mapping fixture see `None` and behave identically.
- Reuses the ADR-0018 seam verbatim: the same `resolve_target_shape` /
  `from_binding` plain-Rust logic the diagnostic corpus drives is now also the
  production path; only the descriptor *source* changed (host argument vs test
  fixture).

**Negative / honest discharge mode.**
- The orphan rule prevents `fossil-hir` from implementing `HirDb` for an
  arbitrary host `Db` with a non-default value; the host must implement `HirDb`
  on ITS own concrete `Db` type and pass the descriptor into
  `resolve_target_shape`. For v0.1 the plain-argument path is the primary
  mechanism, with the `HirDb` trait as the typed carrier hosts implement. The
  tracked `typecheck_mapping` itself still uses `AcceptAll` (it cannot upcast a
  `&dyn fossil_base::Db` to `HirDb`); a future phase that wants the tracked
  query to see a real schema would thread the descriptor through a host db
  wrapper, not widen the query key.

**Neutral.**
- `rudof_iri` is promoted from a dev-dependency to a regular dependency of
  `fossil-hir` (for the `IriS` lookup); it is WASM-clean per rudof-wasm.md.
- The WASM gate grows to 9 crates (adds `fossil-sinks`, `fossil-ide`,
  `fossil-ide-db`) in both `.github/workflows/ci.yml` and `CLAUDE.md`, since
  `fossil-ide` now carries the WASM-clean IDE feature logic (and `lsp-types`).
