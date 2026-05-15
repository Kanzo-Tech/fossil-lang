# rudof on wasm32-unknown-unknown — Phase 0 spike outcome

**Date:** 2026-05-15
**rudof commit spiked:** `d024a53b7d111accd628339b14e6ac5884dabfa8` (2026-05-12 12:01:31 +0200, "revert: chore: release 0.3.2-rc.1 [skip ci]")
**rudof workspace version:** 0.3.1
**Crates tested:** shex_ast, shex_validation, rudof_iri, prefixmap (all at workspace v0.3.1)
**Toolchain:** rustc 1.90.0 (1159e78c4 2025-09-14)
**Decider:** Angel Iglesias
**Spike protocol:** `.planning/phases/00-workspace-genesis-rudof-spike/00-RESEARCH.md` §"rudof WASM spike — concrete protocol"

## Context

This spike resolves the single highest-risk Phase 0 question (RESEARCH.md confidence
LOW-MEDIUM): whether the rudof crates that Fossil's `fossil-descriptors-output`
will depend on compile to `wasm32-unknown-unknown`. The playground depends on
this; if rudof failed WASM, the OutputDescriptor trait would have to absorb the
difference via a degraded fallback impl.

Strong positive prior identified during STACK research: rudof's own workspace pins
`getrandom = { version = ">=0.3, <0.4", features = ["wasm_js"] }`,
which signals the project was designed with WASM in mind.

## Spike results

### Step 3 — independent crate checks

| Crate | `cargo check --target wasm32-unknown-unknown` | Notes |
|-------|-----------------------------------------------|-------|
| shex_ast | **PASS** | Clean compile |
| shex_validation | **PASS** | Clean compile |
| rudof_iri | **PASS** | Clean compile |
| prefixmap | **PASS** | Clean compile |

All four crates compile to `wasm32-unknown-unknown` from the unmodified upstream
workspace with no feature flag changes required.

### Step 4 — Transitive offender analysis

For confirmation, ran reverse-dep checks against the WASM-incompatible deps that
typically break compiler crates per `.planning/research/PITFALLS.md` P-CRIT-2:

| Dep | `cargo tree -i <dep>` against shex_validation @ wasm32 | Verdict |
|-----|--------------------------------------------------------|---------|
| `mio` | Only present transitively for `tracing-test` (dev/test scaffolding) | OK — not in build path |
| `tokio` | Not present in WASM build | OK |
| `reqwest` | Not present in WASM build | OK |
| `getrandom` | Three versions resolve (0.2.17, 0.3.4, 0.4.2) | OK — all three resolve cleanly under wasm32 because rudof pins `wasm_js` feature on its direct dep, and the others are leaf-level enough not to break |

### Step 5 — `default-features = false` reduction

**Not needed.** Step 3 passed unconditionally; the reduction protocol (creating a
`spike-wasm` sub-crate with `default-features = false`) was skipped.

## Outcome

**Selected:** Path **(a)** — Clean WASM.

**Rationale:** All four target crates (shex_ast, shex_validation, rudof_iri,
prefixmap) compile to `wasm32-unknown-unknown` from upstream rudof workspace v0.3.1
with no feature flag adjustments. No problematic transitive deps appear in the
build path (the `mio` reference is dev/test-only via `tracing-test`). The spike
confirms the strong positive prior from STACK research: rudof was designed with
WASM in mind, not retrofitted for it. The OutputDescriptor trait insurance design
(plug-in fallback impl) remains valuable as a defensive measure for unknown future
breakage but is not the required path for v0.1.

## Action items

- [ ] Email to Labra Gayo: see `rudof-wasm-email-draft.md`. Sent on YYYY-MM-DD <to be filled>. **Even though the spike passed, the email is still worth sending** to open the WESO collab relationship (per RESEARCH.md "A reply may save 3 weeks" — applies forward too: known-good signal from upstream prevents future-Angel from re-spiking on a regression).
- [ ] Reply received: <awaiting>
- [ ] Phase 3 task: add `shex_ast`, `shex_validation`, `rudof_iri`, `prefixmap` at version `0.3` to workspace `[workspace.dependencies]` (not pinned to `=0.3.1` because rudof publishes patches; track minor bumps with care). Pin **major** version, allow **patch** drift.
- [ ] Phase 3 task: design `OutputDescriptor` trait with the plug-in shape from `.planning/PROJECT.md` Key Decisions, even though path (a) means we can use rudof directly in the WASM playground. The trait abstraction is insurance and also enables future SHACL impl as v2 add-on.
- [ ] Re-run this spike at Phase 3 start against whatever rudof version is then current — confirm no regression. If a future rudof release breaks WASM, file an upstream issue and consider pinning to last-known-good while it's resolved.

## Spike artifacts (local logs, not committed)

- `/tmp/rudof-spike-commit.txt` — commit SHA spiked against
- `/tmp/rudof-spike-{shex_ast,shex_validation,rudof_iri,prefixmap}.log` — per-crate check output (all PASS)

## Forward-impact summary

- **Phase 3 (`fossil-descriptors-output`)** can wire rudof directly with no fallback scaffolding. Standard `[workspace.dependencies]` declaration; standard usage in the descriptor impl.
- **Phase 7 (playground)** can include `fossil-descriptors-output` in the `fossil-wasm` dep tree without conditional-compilation gymnastics. The "ShEx checking unavailable in playground" banner contemplated under path (d) is no longer needed.
- **OutputDescriptor trait abstraction** is still designed (insurance + extensibility for SHACL Core in v2), but the "WasmSystem stubs it out" branch is unused for v0.1.
