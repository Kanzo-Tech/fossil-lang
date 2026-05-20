# ADR 0010: Make the MIR rewriting engine a plain-Rust fixpoint, not a Salsa-tracked query

**Date:** 2026-05-20
**Status:** accepted
**Decider:** Angel Iglesias Préstamo
**Cite:** `.planning/phases/04-mir-algebra-rewriting-complete-codegen/04-RESEARCH.md` §"Rewriting engine = plain-Rust fixpoint, NOT salsa-tracked" + §"Pitfall 2"; `operator-algebra.md` §4 (R1–R10); ADR-0005 + plan 02-07 (the per-mapping invalidation barrier)

## Context

The operator-algebra optimizer (CORE-09) rewrites the `Op` vec of a `MirGraph`
with the ten algebraic equivalences R1–R10 (`operator-algebra.md` §4): R1–R6
are the structural rules of Min Oo & Hartig (filter/project fusion, project /
filter pushdown, filter-over-union distribution, rename expansion), and R7–R10
are the Fossil-specific typed rules (partial evaluation, static-true/false,
group-by fusion). These rewrites run inside the per-mapping compilation
pipeline: `lower_to_mir(db, mapping)` produces the unoptimised graph and the
optimizer transforms it before codegen consumes it.

Three forces are in tension. First, the Phase 2 invalidation barrier
(ADR-0005, plan 02-07) caps the per-mapping fan-out at
`MAX_PER_MAPPING_FAN_OUT = 1`: a body-only edit to one mapping must re-execute
at most one per-mapping query of each kind. Modelling each rewrite rule as its
own `#[salsa::tracked]` query would add up to ten new per-mapping tracked
queries, multiplying the fan-out and breaking the barrier. Second, rewrite
rules can fail to terminate if they oscillate — R3 (push project below filter)
has a structurally inverse shape, and a non-idempotent rule can re-fire forever
(RESEARCH Pitfall 2). Third, the CLAUDE.md hard rule forbids `Box<dyn Trait>`
inside Salsa queries, so the rules must dispatch by enum/match, not trait
objects.

We considered three engine shapes: a per-rule tracked query, a single new
`optimize` tracked query wrapping `lower_to_mir`, and a plain-Rust function
called from within `lower_to_mir`.

## Decision

We will implement rewriting as a **plain-Rust** function,
`fn rewrite<'db>(db: &'db dyn fossil_base::Db, graph: MirGraph<'db>) -> MirGraph<'db>`,
that applies the rules to a **fixpoint** — it loops over the rule list R1..Rn,
re-running until a full pass fires no rule. It is **not** a `#[salsa::tracked]`
query and adds **zero** new tracked queries. It is called once from the tracked
`lower_to_mir` frame, just before that query returns its `MirGraph`, so its
result is memoised together with `lower_to_mir`.

Termination is guaranteed by three mechanisms: (a) **canonical single-direction
push** — R3 and R5 push project / filter DOWN toward sources only, with no
inverse rule, so they cannot ping-pong; (b) **idempotent rules** — each rule's
trigger pattern no longer matches its own output (R1 leaves one Filter, R6
leaves no Rename, the R3/R5 free-variable guards become unsatisfiable once the
canonical form is reached), unit-tested as `rewrite ∘ rewrite == rewrite`; and
(c) an **iteration cap** `MAX_ITERS = 100` that, on overrun, calls
`fossil_base::delay_span_bug` naming the non-converging rewrite and returns the
partially-rewritten graph (degrade, never hang).

Rules dispatch by `match` over the `Op` enum (no trait objects). `Box<Expr>`
recursion inside rule bodies is data, not a trait object, so the no-`Box<dyn
Trait>` rule is upheld. Index discipline (`Op` `input`/`left`/`right` are
`usize` indices into the topo-ordered op vec) is maintained inside the engine
by helpers that renumber consistently when a rule inserts or removes an op.

## Consequences

The per-mapping tracked-query count is **unchanged** — rewriting adds no new
tracked query, so `MAX_PER_MAPPING_FAN_OUT = 1` and the invalidation regression
total (18) stay intact. The optimizer is a pure structural transform that is
trivial to unit-test in isolation (direct `MirGraph` construction per ADR-0009),
and idempotency is verified per rule (SC#2). Because `rewrite` is plain Rust, it
can be called from any future caller (e.g. a CLI `--no-optimize` flag could skip
it) without a Salsa-query refactor.

The cost is that rewriting is **not independently memoised**: if two different
mappings produce structurally identical unoptimised graphs, each pays the
rewrite cost (it is not deduplicated by Salsa interning). For per-mapping graphs
of a handful of ops this is negligible, and the alternative would reintroduce
the fan-out problem. The iteration cap means a buggy future rule degrades to a
diagnostic rather than a hang — acceptable, but it shifts the failure mode to a
delayed bug that must be caught by the per-rule idempotency tests.

### Alternatives considered

- **Each rule as its own `#[salsa::tracked]` query.** Rejected: it multiplies
  the per-mapping query count by up to ten and directly threatens the ADR-0005
  invalidation barrier — the single most load-bearing performance invariant of
  the compiler.
- **A separate `optimize` tracked query wrapping `lower_to_mir`.** An acceptable
  fallback: it adds exactly one tracked query (not ten) and would memoise
  rewriting independently. Rejected for v0.1 in favour of inlining into
  `lower_to_mir`, which adds zero queries; we can promote rewriting to its own
  tracked query later if independent memoisation proves worthwhile, without
  changing the rule implementations.
