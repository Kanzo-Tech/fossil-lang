# ADR 0009: Define all 11 MIR operators; lower 4 from source, test 7 via direct construction

**Date:** 2026-05-20
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:** `.planning/phases/04-mir-algebra-rewriting-complete-codegen/04-RESEARCH.md` (reachability gap); `operator-algebra.md` §2-3; Phase 3 SUMMARY deferred-item #6 (Pratt-lowered HIR deferred to "Phase 4+")

## Context

`operator-algebra.md` §2 defines an 11-operator typed algebra (`Source`,
`Project`, `Extend`, `Rename`, `Filter`, `Join`, `Union`, `GroupBy`,
`Aggregate`, `Distinct`, plus the two typed-sink refinements `TripleEmit` and
`Sink`). CORE-08 requires the MIR to be the complete typed conservative
extension of the Min Oo & Hartig algebra.

The surface language, however, is much narrower at this point. `HirExpr` has
exactly four leaf variants (`Template`, `FieldRef`, `StringLit`,
`PrefixedName`) — there is no `Pipeline`, `Call`, `BinOp`, closure, or any
combinator surface syntax. Phase 3's checker (plan 03-05) explicitly did NOT
add a `seq.filter` stub for this reason, and Phase 3's SUMMARY deferred-item #6
defers the full Pratt-lowered `HirExpr` tree to "Phase 4+".

Consequently only 4 of the 11 operators are *reachable from `.fossil` source*:
`Source` (the `from` binding), `Extend` (the `iri = ...` computed column),
`TripleEmit` (one per `prefix:local = ...` property), and `Sink` (the GraphAr
terminal). The other 7 operators have no surface syntax that lowers into them.

The forces in tension: CORE-08 demands the complete IR; but building the full
Pratt-lowered `HirExpr` tree (Pipeline/Call/closure) now is the
highest-risk addition on the Phase 4 critical path — it threatens the
walking-skeleton invariant (`fossil compile examples/hello.fossil` must stay
byte-identical) and balloons the phase's scope.

A second, smaller shape question rides along: how to represent the R9
empty-source rewrite target. The options are a dedicated `Op::Empty { schema }`
variant or a `Source` reused with an empty marker.

## Decision

We will:

1. **Define all 11 `Op` variants now**, plus `Op::Empty`. CORE-08 is a statement
   about the IR contract, not about surface coverage — algebra completeness
   lives at the MIR layer. The full enum is the contract that every later
   Phase 4 plan (rewriting, codegen, optimisation) builds on.

2. **Lower only the 4 source-reachable operators** (`Source`, `Extend`,
   `TripleEmit`, `Sink`) in `lower_to_mir`. This is the entire surface-driven
   path for v0.1.

3. **Test the other 7 operators** (`Project`, `Rename`, `Filter`, `Join`,
   `Union`, `GroupBy`, `Aggregate`, `Distinct`) via DIRECT
   `MirGraph::new(db, vec![...])` construction snapshot tests in plans
   04-04/04-05 — the same precedent as Phase 3's helper-proving of the ShEx
   backward-check logic (plan 03-08), which drove SC#2/SC#4 through the public
   helper surface rather than a faked end-to-end path.

4. **NOT build the full Pratt-lowered `HirExpr` tree in Phase 4.** Surface
   pipeline syntax is DEFERRED to a future phase, gated behind its own spike.

5. **Represent the R9 empty-source target as a dedicated `Op::Empty { schema }`
   variant**, carrying the column schema it would have produced so codegen can
   emit a correctly-shaped `SELECT ... WHERE false` / `LIMIT 0` shell. The
   alternative — reusing `Source` with an empty marker — was rejected because
   it muddies the codegen match and the snapshots (a `Source` that produces no
   rows is a special case threaded through every downstream operator's
   reasoning), whereas a distinct variant is self-documenting and keeps
   `schema_of` total.

## Consequences

**Easier.** The IR is complete and stable from the start of Phase 4: the
rewriting engine (04-02/04-03), codegen (04-04/04-05), and the
erase-types-≡-untyped-algebra check (04-07) all build against the final
12-variant `Op` enum and the typed `Expr` ADT. `schema_of` is total over all
operators. The walking-skeleton invariant is preserved — `lower_to_mir` only
ever emits the 4-op shape `hello.fossil` produced in Phase 1, byte-identical.

**Harder / new risk.** The 30-mapping corpus (SC#1) is a *MIR-level* corpus —
mostly hand-constructed `MirGraph`s — not a `.fossil`-source corpus, because
most operators cannot be reached from source yet. Plans state this explicitly.
The risk is that direct-MIR tests can drift from what the (future) surface
syntax would actually lower into; mitigated by keeping the direct construction
faithful to `operator-algebra.md` §3 and by the eventual surface-syntax phase
re-driving the corpus from source.

**Neutral.** A future phase adds surface pipeline syntax (Pipeline/Call/closure
in `HirExpr` + the Pratt-lowered tree) to make the corpus mappings
source-driven. Until then, the 7 unreachable operators exist in the IR and are
exercised structurally, but no `.fossil` program reaches them.
