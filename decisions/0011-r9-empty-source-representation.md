# ADR 0011: Represent R9's empty relation as a dedicated `Op::Empty { schema }`

**Date:** 2026-05-20
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:** `operator-algebra.md` §4.2 (R9); `.planning/phases/04-mir-algebra-rewriting-complete-codegen/04-RESEARCH.md` Open Question 1; ADR-0009 (all 11 MIR ops defined)

## Context

Rewrite rule R9 (`operator-algebra.md` §4.2) rewrites a statically-false
filter to an empty relation: `filter(s, p) where p is statically false ≡
empty(schema(s))`. Before Phase 4 the MIR `Op` enum had no way to represent an
empty relation — every node carried row data forward from a `Source`. R9 needs
a target node, and that node must preserve the column schema the filtered
relation *would* have produced so that downstream operators (`TripleEmit`,
`Sink`) still see the correct column names and codegen emits SQL of the right
shape (an empty result set, not a malformed one).

Two representations were on the table. (a) A dedicated `Op::Empty { schema:
Vec<SmolStr> }` variant: R9 replaces the `Filter` in place with `Op::Empty`
carrying `schema_of(input)`, and codegen (plan 04-04) emits a `SELECT
<typed-null cols> WHERE false` (or `SELECT * FROM <input> LIMIT 0`) shell that
keeps the declared schema. (b) Reuse `Op::Source` with an empty-relation marker
(e.g. an empty-URI sentinel or a boolean flag), so no new variant is needed.

This is an alternatives-decision (which IR shape) and was flagged ADR-worthy in
the Phase 4 research (Open Question 1).

## Decision

We will represent R9's output as a dedicated `Op::Empty { schema: Vec<SmolStr>
}` variant (added to the `Op` enum in plan 04-01). R9 replaces the
statically-false `Filter` in place with `Op::Empty { schema:
schema_of(db, ops, input_of_filter) }`, leaving the node index unchanged so
consumers need no rewiring. The schema is carried on the node so codegen
(plan 04-04) can emit a `SELECT … WHERE false` / `LIMIT 0` shell preserving the
declared column names; `schema_of` already returns `Op::Empty`'s declared
`schema` directly, so schema propagation through an empty node is well-formed.
ADR-0009 already records that `Op::Empty` exists as one of the defined-but-
mostly-direct-construction ops; this ADR is the focused record of *why* it is a
distinct variant and how it is produced and consumed.

## Consequences

Positive: codegen for the empty case is clean and local (one `Op::Empty` arm,
no `Source`-with-flag branching in every place that pattern-matches a
`Source`); the MIR snapshot for R9 reads unambiguously (`Empty { schema: [...]
}` rather than a `Source` with a sentinel that a reader must decode); R9 is
trivially idempotent because `Op::Empty` carries no predicate, so the rule
cannot re-fire on its own output.

Negative: one more `Op` variant to handle in every exhaustive match over the
algebra (`schema_of`, `map_indices`, `references`, codegen). This is a small,
bounded cost — the matches are already exhaustive and the compiler enforces
coverage.

Neutral: `Op::Empty` is currently produced *only* by R9 (it is not lowered from
`.fossil` source); should a future surface form ever denote an empty relation
directly, this variant is the natural target.

## Alternatives considered

Reuse `Op::Source` with an empty-relation marker — rejected. It conflates a
data origin with an optimizer-synthesised empty result, forces every
`Source`-matching site to decode the marker, muddies codegen (a `Source` arm
that sometimes emits `WHERE false`), and produces a less legible snapshot.
A dedicated variant keeps the optimizer's output structurally distinct from
genuine sources.
