# ADR 0008: Real per-mapping spans via a `Spans<'db>` side table

**Date:** 2026-05-19
**Status:** accepted
**Decider:** Angel Iglesias (Fossil core)
**Cite:**
- `.planning/phases/03-bidirectional-type-checker-shex-target/03-RESEARCH.md` §"Architecture Patterns Pattern 2"
- Phase 2 plan 02-06 RESEARCH §Q5 (provenance side-table choice — same rationale)
- ADR-0005 (`ItemTree` signatures vs. `body()` per-mapping invalidation barrier)
- Plan 02-07 `mapping_cst_node` intermediate query (real per-mapping invalidation barrier; `MAX_PER_MAPPING_FAN_OUT = 1`)

## Context

Phase 2 deferred real spans for non-literal expressions and used zero-width
`Span { start: 0, end: 0 }` placeholders for the literal subset in
`crate::provenance::Provenance::span`. The blame STRUCTURE was deliverable
(plan 02-06's `compatible()` two-span pattern with `delay_span_bug`) but the
byte ranges weren't actionable — pointing at character 0 of the file isn't
useful for the LSP host or the CLI's miette renderer.

Phase 3's bidirectional checker (`crate::check::compatible` graduating in
plan 03-05) needs REAL spans for diagnostics. Phase 3 success criteria SC#1
("a non-existent column produces a compile error pointing at the use site"),
SC#2 ("provides Optional vs requires required — point at BOTH the mapping
body and the shape constraint that demanded cardinality 1+"), and SC#4
("OneOf rejection names the shape constraint, shows disjuncts, generates
the split-into-N suggestion") ALL depend on the diagnostic underline
pointing at the right byte range. Without real spans the user can't act on
the message.

Three options for plumbing real spans were considered:

1. **Add a `span: Span` field to every `HirExpr` variant.** Mechanically
   simple — at lowering time, each `lower_expr` arm records its `node.text_range()`
   and stores it on the produced `HirExpr`. Downstream consumers
   (`check.rs`, hover) read `expr.span` directly.

2. **Per-mapping `Spans<'db>` side table keyed by `(MappingLoc, ExprId)`,
   populated by a separate `#[salsa::tracked]` query.** The lowering code
   stays span-free; a new `crate::spans::spans(db, mapping)` query walks
   the same CST as `crate::body::body` and emits `(ExprId, Span)` pairs.

3. **On-demand span lookup at diagnostic emission time via CST walk.** No
   side table at all — when a diagnostic needs a span, walk
   `mapping_cst_node(db, mapping)` to re-find the relevant node and read
   its `text_range()`.

The forces in tension:

- **Salsa interning.** Phase 2 (plan 02-04 / ADR-0005) established that
  `HirExpr` is interned by structural equality — two source-identical
  expressions at different source positions dedupe in the interner,
  preserving the "structural equality at the HIR layer → pointer equality
  after Salsa interning" contract. This is load-bearing for the
  bidirectional checker's per-type-equality fast path and for the LSP
  hover's pointer-comparison-based caching.

- **Per-mapping invalidation barrier.** Plan 02-07 fixed a leak where
  `body()` depended on `parse(db, file)` directly, causing all 10 mappings'
  body queries to re-execute on a single-mapping body edit. The fix —
  `mapping_cst_node(db, mapping)` — sits between `parse()` and `body()` as
  a rowan-`GreenNode`-Arc-sharing-aware barrier (sibling green subtrees
  stay bit-identical → Salsa `maybe_update` returns `false` → downstream
  body queries validate via `DidValidateMemoizedValue` instead of
  re-executing). The invariant is enforced by
  `crates/fossil-hir/tests/invalidation_regression.rs`'s
  `MAX_PER_MAPPING_FAN_OUT = 1` assertion.

- **Diagnostic emission cost.** Diagnostics are emitted from inside the
  `#[salsa::tracked]` checker queries (`compatible()` in plan 03-05 calls
  `delay_span_bug` from within `typecheck_mapping`). Re-walking the CST
  every time a diagnostic emits a span would duplicate work the lowering
  arena already does once per body.

## Decision

We will use **Option 2** — a per-mapping `Spans<'db>` side table populated
by a separate `#[salsa::tracked]` query (`crate::spans::spans(db, mapping)`)
that reads `mapping_cst_node(db, mapping)`.

`HirExpr` stays unchanged — no new span field. `Spans<'db>` is a Salsa
tracked struct holding `Vec<(ExprId, Span)>`, populated during the per-
mapping spans query by walking `MAPPING_BODY > PROPERTY > EXPR` and
recording each property RHS's `text_range()` (descending into the first
non-trivia EXPR child to get a tight span without surrounding whitespace).

The Phase 2 `crate::provenance::Provenance::span` is now populated from
`spans(db, mapping).get(expr_id)` instead of `Span { start: 0, end: 0 }`.
This is invisible to the Phase 2 `compatible()` API (plan 03-05 graduates
the stub to real subtyping) — the upgrade is purely a data-flow change at
the provenance side table's input.

## Consequences

**Positive:**

- `HirExpr` PartialEq/Hash UNCHANGED. Salsa interning's structural-
  equality-to-pointer-equality contract holds. The 50–100× cardinality
  blow-up that Option 1 would have caused (per Phase 2 RESEARCH §Q5's
  argument — one Ty entry per AST position instead of one per shape) is
  averted, for spans this time.

- Phase 2's `MAX_PER_MAPPING_FAN_OUT = 1` invariant continues to hold.
  `spans()` reads `mapping_cst_node`, NOT `parse(file)`, so it inherits
  the same Arc-shared-subtree invalidation barrier ADR-0005 + plan 02-07
  established. The invalidation regression test extends to assert
  `spans_count == 1` after a single-mapping body edit; passes.

- Same architectural pattern as Phase 2's provenance side table — the
  side-table pattern is now consistent across the type / span /
  provenance side tables. Phase 3+ can use the same pattern for any
  future per-expression metadata (effects, region, refinement,
  closure-rendering) without re-litigating the design choice.

- The lowering arena stays simple — no new mutable context threaded
  through every `lower_expr` arm. Plan 03-04's implementation is
  ~150 LOC of new code in `crates/fossil-hir/src/spans.rs` plus a
  single-line refactor in `crate::provenance::expr_types`. No
  cross-cutting churn through `crate::lower`.

**Negative:**

- `Spans<'db>` is a separate `#[salsa::tracked]` query, so the
  `invalidation_regression.rs` test threshold `MAX_REEXECUTIONS` bumps
  from 16 → 17 (the +1 is `spans(M_3)` re-executing after the body edit).
  Acceptable: the LOAD-BEARING `MAX_PER_MAPPING_FAN_OUT = 1` invariant
  stays at 1 (siblings DO NOT re-execute).

- Spans are mapping-relative (rowan's `SyntaxNode::new_root(green)` resets
  offsets to zero on the per-mapping subtree). The diagnostic-emission
  layer (plan 03-05's `compatible()` and the LSP host) is responsible
  for converting mapping-relative offsets to file-absolute when needed.
  This is a one-time-per-diagnostic cost (resolve the mapping's absolute
  start offset by walking `parse(file)` and add). Mitigated by the fact
  that the LSP host already walks `parse(file)` to find the hover
  position.

- `Spans::get(expr_id)` is a linear scan (`Vec<(ExprId, Span)>`).
  Acceptable for Phase 3 (typical mapping has ≤50 expressions; LSP hover
  is one lookup per request). If profiling later shows hot, swap for
  `HashMap<u32, Span>` or sorted-Vec + binary search — the public API
  shape (`spans(db, m).get(db, expr_id)`) stays unchanged.

- Option 3 (on-demand) was rejected because it duplicates the lowering's
  CST walk every time a diagnostic is emitted. The lowering arena is the
  right point to record positions because it's already navigating the
  CST node-by-node. The side-table caches the result so subsequent
  diagnostic emissions don't pay for re-walking.

**Neutral:**

- Plan 03-04 retains the Phase 2 ExprId convention (`ExprId(i)` = i'th
  property's RHS expression). Phase 2's `HirExpr` is non-recursive
  (Template/FieldRef/StringLit/PrefixedName are all leaf forms), so per
  the plan EVERY `HirExpr` has an `ExprId`. Subexpression-level ExprId
  allocation (for nested function calls, ternaries, etc.) lands when
  the Pratt-lowered expression tree extends `HirExpr` in a later plan.

- Phase 3 plan 03-05's `compatible()` wider blame (`BlamePos` enum +
  cardinality) reads from this Spans side table for both source and
  destination blame positions. No further span infrastructure work in
  Phase 3 — this ADR closes the spans story for Milestone 1.
