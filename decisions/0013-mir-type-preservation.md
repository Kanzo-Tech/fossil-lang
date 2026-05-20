# ADR 0013: Treat MIR types as erasable — codegen ignores them (type preservation)

**Date:** 2026-05-20
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** operator-algebra.md §1 / §9 (conservative-extension claim); type-system.md §3 (schema propagation); 04-RESEARCH.md §"SC#3 type preservation"; plan 04-07 (SC#3)

## Context

Fossil's mid-level IR is a *typed* operator algebra: every `Op` and `Expr` may
carry a `Ty<'db>` annotation (`Source.row_type`, `AggSpec.ty`, `Expr::Call.ty`,
`Expr::BinOp.ty`). `operator-algebra.md` §1 frames this typed algebra as a
**conservative typed extension** of Min Oo & Hartig's *untyped* relational
algebra: the eleven operators have exactly the operational meaning of their
untyped counterparts, and the type layer is additive — it gates compile-time
errors and selects the typed rewrites R7–R10, but it does not change *what the
program computes*.

For the OSS paper (§5 Compilation) and for the milestone's correctness story
("if it compiles, the graph is well-formed") we need this claim to be more than
prose. Two questions are in tension:

1. **Does codegen depend on types?** If the SQL the compiler emits varied with
   the type annotations, the typed algebra would NOT be a conservative extension
   — types would carry operational meaning, and erasing them could change
   results. The whole "types are a static gate, not a runtime semantics" story
   would collapse.
2. **How do we *know* the schema-propagation argument is sound** rather than
   asserted? A typed DAG is well-typed if each operator's input schema matches
   its signature; we want a mechanical witness that this holds and that the
   untyped projection agrees.

We considered (a) an *untyped* MIR with a separate side-table type-check, and
(b) letting types influence codegen (e.g. emitting different SQL for an integer
vs string column). Both were rejected (see Decision).

## Decision

We will treat the `Ty` annotations on MIR as **operationally erasable**: codegen
reads NO type for operational semantics, and the generated SQL is byte-identical
with or without the annotations.

Concretely:

- Every `ty:` field is ignored in `fossil-codegen` — `render_expr` matches
  `Call { ty: _, .. }` / `BinOp { ty: _, .. }`, and the aggregate arm reads only
  `agg_fn` / `in_field` / `out_field`, never `AggSpec.ty`.
- `fossil_mir::erase_types(db, g)` produces the *untyped projection*: it replaces
  every `Ty` with a single interned sentinel (`TyKind::Unknown(InferenceId(u32::MAX))`)
  while leaving every structural field untouched.
- The property test `fossil-codegen/tests/type_preservation.rs` asserts
  `codegen(g).sql == codegen(erase_types(g)).sql` over the entire 30-mapping
  corpus (SC#3). It is the executable witness of the claim.

**The preservation argument (structural induction over the MIR DAG).** Let a
node be *well-typed* when its output schema is the one its operator's signature
assigns given well-typed inputs (type-system.md §3, schema propagation). Base
case: a `Source` node's output schema is its descriptor-derived `row_type` —
well-typed by construction. Inductive step: for each operator
(`Project`/`Extend`/`Rename`/`Filter`/`Join`/`Union`/`GroupBy`/`Aggregate`/
`Distinct`/`TripleEmit`/`Sink`), if its input node(s) are well-typed then the
operator's signature determines a well-typed output schema (the bidirectional
checker, Phase 3, is exactly the procedure that verifies the side conditions —
free columns present, join key types compatible, aggregate input numeric, …).
Since the DAG is finite and acyclic (indices reference only lower-numbered ops),
induction over topological order shows every node is well-typed. Crucially, the
*operational* meaning of each operator is its untyped relational-algebra meaning;
the type only certifies the side conditions. Therefore the untyped projection
(erase the types) denotes the same relation, and — because codegen is a
syntax-directed function of the structural fields alone — emits the same SQL.
`erase_types` + the corpus property test mechanize the conclusion of this
induction.

## Consequences

- **Types are a pure static gate.** They are erasable at codegen; the
  "conservative typed extension" claim from operator-algebra.md §1/§9 is now
  backed by a test that fails the instant any codegen path starts reading a
  `Ty`. This is the SC#3 deliverable and supports the paper's §5 claim.
- **R7–R10 are meaning-preserving optimizations**, not semantic changes: they
  rewrite the typed DAG (constant-fold an `Extend`, drop a statically-true
  `Filter`, turn a statically-false `Filter` into `Op::Empty`, fuse `GroupBy`
  keys) but the untyped projection of the rewritten graph denotes the same
  relation as the original — so the erase-invariant holds across rewrite
  outcomes too (the corpus carries 10 post-rewrite graphs and the property test
  covers them).
- **The sentinel never reaches SQL.** Erased types are an internal MIR marker;
  codegen ignores types, so invariant #8 ("no `TyKind::Unknown` in generated
  SQL") is upheld — the corpus no-leak guard checks this independently.
- **Maintenance cost:** any future codegen feature that genuinely needs a type
  (e.g. a datatype-driven `CAST`) would break the erase-invariant. That is the
  intended tripwire: such a feature must either be expressed structurally (a
  `Call`/`Extend` node the parser/lowering emits, carrying the cast in its
  shape) or it must be accompanied by a deliberate revision of this ADR and the
  conservative-extension claim. The test makes the decision visible.

## Alternatives considered

- **Untyped MIR + separate type-check side table.** Rejected: it loses the
  typed-rewrite leverage (R7–R10 query types directly) and splits the IR into
  two artifacts that can drift; the typed MIR with erasable types keeps a single
  source of truth while still permitting the untyped projection on demand.
- **Types affecting codegen** (datatype-specialised SQL). Rejected for v0.1: it
  breaks the conservative-extension claim and the "types are a static gate"
  story. Datatype-driven behaviour, when needed, is modelled structurally
  (explicit cast/`Call` nodes) so codegen stays type-blind.
