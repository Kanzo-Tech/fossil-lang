# ADR 0006: Dispatch output descriptors via an `OutputDescriptorKind` enum and a `SystemWithDescriptors` extension trait, and carry suggestion source on a structured `Diagnostic` field

**Date:** 2026-05-19
**Status:** accepted
**Decider:** Angel Iglesias Préstamo (kanzo.tech)
**Cite:** `.planning/phases/03-bidirectional-type-checker-shex-target/03-RESEARCH.md` (§"Architecture Patterns Pattern 1", §"Common Pitfalls Pitfall 2", §"ADR Candidates"); ADR-0003 (Db trait + System abstraction); `decisions/rudof-wasm.md`; plan 03-03 (Task 2); plan 03-08 SC#4 second-order test (informs the `suggestion_source` choice).

## Context

ADR-0003 routes descriptor access through the host-provided `System`
abstraction: hosts hand the type-checker a `&dyn OutputDescriptor` trait
object. That works fine OUTSIDE Salsa — the playground UI listing loaded
descriptors, the CLI printing descriptor info, tests constructing mock
descriptors. But the Phase 3 bidirectional checker (plan 03-05) runs inside
`#[salsa::tracked] fn typecheck_mapping`, and Salsa 0.26 cannot intern or
memoize trait objects (`Box<dyn OutputDescriptor>`): they lack structural
equality. This is captured in the project's "no `Box<dyn Trait>` inside
Salsa queries" hard rule (`CLAUDE.md`).

So `typecheck_mapping` needs descriptor dispatch through CONCRETE types.
Three knobs are in tension:

1. **How to expose the concrete-type dispatch surface to `fossil-hir`.**
   Either downcast a `&dyn OutputDescriptor` to its concrete impl inside
   the query body (fragile, requires `Any` trickery), or introduce an enum
   that holds the concrete types directly.

2. **Where to put the host accessor `output_descriptor_kind() ->
   &OutputDescriptorKind`.** Two options:
   - **Option A:** add the method to `fossil-base::System` directly.
   - **Option B:** add it to a new EXTENSION TRAIT
     `SystemWithDescriptors` living in `fossil-descriptors-output`.

   Topology check at plan-03-03 execution time (2026-05-19): the natural
   dependency direction is `fossil-descriptors-output → fossil-base`
   (descriptors are a concrete impl of base abstractions). Option A would
   INVERT this and force `fossil-base` to know about
   `OutputDescriptorKind`. While that doesn't introduce an immediate
   cycle (fossil-base wouldn't import fossil-descriptors-output for the
   accessor signature; it would forward-declare the type as `<T: ?>`), it
   couples the substrate to a specific descriptor variant catalogue.
   Future cycle risk: as `fossil-descriptors-output` adds more types and
   integrations (Phase 4 facets, Phase 5 SHACL via DESC-01 v2 add-on), it
   may NEED to depend on `fossil-base` types beyond the descriptor surface.
   If we'd added the accessor to `fossil-base::System`, every such future
   import would create a cycle.

3. **How to carry code-suggestion source text alongside a diagnostic.**
   Phase 3's ShEx `OneOf` rejection (SC#4) emits a `Diagnostic` AND a
   generated Fossil source split-into-N-mappings snippet. Plan 03-08's
   `shex_one_of_split_suggestion_compiles` second-order test extracts the
   snippet and feeds it back through `parse → lower → typecheck_mapping` to
   prove "the suggestion compiles". Two carrier shapes:
   - **Approach A (structured):** add a typed
     `suggestion_source: Option<String>` field to `Diagnostic`.
   - **Approach B (Markdown-embedded):** stash the snippet inside the
     diagnostic's `message` text with a delimiter (e.g. `\n---\n`); consumers
     parse it out.

The trait surface IS genuinely useful outside Salsa. We don't want to drop
it — we want a parallel concrete-type API for inside-Salsa use.

## Decision

We will adopt all three sides of the decision together:

1. **Introduce `OutputDescriptorKind` enum** in
   `fossil-descriptors-output::kind` with variants `ShEx(ShExDescriptor)` and
   `AcceptAll(AcceptAllDescriptor)`. `fossil-hir`'s `typecheck_mapping`
   dispatches via `match` on this enum. The `OutputDescriptor` trait stays
   as the outside-Salsa surface API.

2. **Mandate Option B** for the host accessor. A new extension trait
   `SystemWithDescriptors: fossil_base::System` lives in
   `fossil-descriptors-output::system_ext`. Its single method
   `output_descriptor_kind(&self) -> &OutputDescriptorKind` has a default
   impl returning `&OutputDescriptorKind::ACCEPT_ALL_DEFAULT` (an inherent
   `const` on the enum). Concrete host types (`fossil-cli::CliSystem`,
   `fossil-wasm::WasmSystem`) implement BOTH traits explicitly.
   `fossil-base::System` stays UNCHANGED.

3. **Adopt Approach A** for suggestion-source carriage. `fossil-base::Diagnostic`
   gains a structured `suggestion_source: Option<String>` field with a
   `Diagnostic::with_suggestion_source(...)` builder. The ShEx `OneOf`
   rejection emitter (plan 03-05) populates this with
   `fossil_descriptors_output::generate_split_suggestion(...)` output. Plan
   03-08's second-order test reads `diag.suggestion_source.as_deref()`
   directly — NO Markdown delimiter parsing.

`AcceptAllDescriptor` is locked as a `pub struct AcceptAllDescriptor;` unit
struct so the `ACCEPT_ALL_DEFAULT` inherent const is const-evaluable. A
compile-time test (`accept_all_default_is_const_constructible`) enforces
this; if a future variant breaks const-constructibility the test fails to
compile.

## Consequences

### Positive

- **No `Box<dyn Trait>` inside Salsa queries.** The `CLAUDE.md` hard rule is
  structurally honoured: `typecheck_mapping` works against concrete enum
  variants, never trait objects.
- **`fossil-base` stays a pure substrate.** It does not know about
  `OutputDescriptorKind` or any specific descriptor type. Future cycle risk
  is eliminated — `fossil-descriptors-output` can grow new `fossil-base`
  deps freely.
- **SC#5 (plug-in-replaceable descriptor) is structurally satisfied.** The
  swap surface lives in the host's `SystemWithDescriptors` impl; switching
  from `AcceptAll` to `ShEx` (or, eventually, `Shacl`) happens at host
  wiring time without touching `fossil-hir`. The
  `output_descriptor_kind_swap_does_not_require_fossil_hir_change` test
  exercises the match-arm shape.
- **Structured `Diagnostic.suggestion_source` is type-safe.** Plan 03-08's
  second-order test reads a typed `Option<String>` — no brittle
  Markdown-string parsing. The same field is reusable by any future
  suggestion-emitting diagnostic (Phase 5 facet rewrites, Phase 6 LSP code
  actions).
- **Walking-skeleton invariant intact.** The CLI's default
  `output_descriptor_kind()` returns AcceptAll, so the `hello.fossil` demo
  still compiles end-to-end (no ShEx schema needed).
- **WASM gate intact.** Descriptors stay WASM-clean; Phase 0 spike's path-a
  outcome is preserved.

### Negative

- **New variants of `OutputDescriptorKind` require updating every match arm
  in `fossil-hir`.** When SHACL or a future descriptor lands (REQUIREMENTS.md
  DESC-01 v2 add-on), the bidirectional checker's match needs an extra arm.
  This is acceptable: adding a new output descriptor is a major
  architectural change that warrants this churn — and the exhaustiveness
  check the compiler enforces is the FEATURE, not a cost.
- **Callers of `output_descriptor_kind()` must `use
  fossil_descriptors_output::SystemWithDescriptors;`** to bring the
  extension method into scope. Minor ergonomic cost; standard Rust pattern.
  Plan 03-05's `typecheck_mapping` doc-comment will name this `use` line
  explicitly.
- **Three modules added to `fossil-descriptors-output`** (`kind`, `shex`,
  `system_ext`). The trait `OutputDescriptor` keeps its place at the crate
  root; the new modules are clearly Phase 3 additions.
- **`Diagnostic` struct literal sites need an explicit `suggestion_source:
  None` line.** Phase 2's parser (`fossil-syntax/parser/diag.rs`) and
  `fossil-base::delay_span_bug` were updated in plan 03-03 Task 2 step 7;
  future callers can use `Diagnostic::new(...)` to avoid the explicit
  `None`.

### Neutral

- The plan-03-02 reservation in `decisions/README.md` for ADR-0006 is now
  filled; the row is promoted from `_(reserved)_` to `accepted` with this
  ADR's date.
- **Future extension:** if SHACL output descriptor lands as the v2 add-on
  (REQUIREMENTS.md DESC-01), add `Shacl(ShaclDescriptor)` to the enum and
  update every match in `fossil-hir`. ADR will reference this one.

## Sources

- 03-RESEARCH.md §"Architecture Patterns" (Pattern 1) + §"Common Pitfalls"
  (Pitfall 2) + §"Bidirectional Checker Shape" (CORE-07 plug-in
  replaceability).
- ADR-0003 (Db trait + System abstraction) — supersedes the implicit "all
  descriptor access is via `&dyn`" assumption.
- ADR-0005 (ItemTree signatures vs. body separation) — analogous shape of
  decision: keep the substrate trait minimal, push concrete types into the
  consuming layer.
- `decisions/rudof-wasm.md` (Phase 0 spike) — informs why descriptors might
  need to swap (degraded fallback to AcceptAll on WASM regression).
- Plan 03-08 SC#4 second-order test (informs `suggestion_source` =
  structured, not Markdown-embedded).
- `crates/fossil-descriptors-output/src/kind.rs::tests::accept_all_default_is_const_constructible`
  (compile-time gate for Serious #8 mitigation).
