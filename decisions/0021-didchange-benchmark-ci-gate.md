# ADR 0021: Gate the didChange perf budget with a MARGINED wall-clock test, keep Criterion ADVISORY

**Date:** 2026-05-22
**Status:** accepted
**Decider:** Angel Iglesias (plan 06-09)
**Cite:** `.planning/phases/06-cli-complete-lsp/06-RESEARCH.md` §SC#2 / Pitfall #1 / Open Question #1; `crates/fossil-lsp/tests/didchange_budget.rs`; `crates/fossil-lsp/benches/lsp_didchange.rs`

## Context

SC#2 sets a performance target: the LSP `didChange` round-trip — `set_text`
(the Salsa `Setter` revision bump) → `def_map` → `typecheck_mapping` over every
mapping → drain the `Diagnostic` accumulator — should complete in under 100ms on
a 200-line Fossil file, so editing feels instant. Two forces are in tension.

First, we want a HARD CI gate so a real regression (an accidentally O(n²) pass,
a lost Salsa memoisation, a per-keystroke full re-parse of every mapping) fails
the build rather than silently degrading the editor. Second, a naked `assert
elapsed < 100ms` on a shared GitHub-hosted runner FLAKES: cold caches, noisy
neighbours, and unoptimised `cargo test` debug builds routinely push a 0.12ms
warm operation into tens of milliseconds and occasionally past any tight
threshold. A flaky gate trains the team to ignore it — worse than no gate.

Criterion is the right tool for tracking the tight target and detecting small
regressions, but its statistical baseline + 20%-regression comparison is only
trustworthy on a stable CPU with no neighbour noise (a pinned or self-hosted
runner). On shared CI its variance swamps a 20% signal.

The canonical 200-line fixture this measures over did not previously exist
(Research Wave 0 gap / Phase-0 Open Question #4); it is authored alongside this
decision (`tests/fixtures/canonical_200.fossil`).

## Decision

We will split the SC#2 enforcement into a HARD gate and an ADVISORY benchmark.

The HARD CI gate is a MARGINED wall-clock correctness test
(`tests/didchange_budget.rs`, a plain `#[test]` run by `cargo test`). It asserts
the worst measured `didChange` round-trip over the canonical fixture stays under
a GENEROUS budget (400ms — well above the 100ms design goal). The margin absorbs
CI noise + debug-build overhead while still catching algorithmic blow-up (an
asymptotic regression on a 200-line file is seconds, not a few ms, so it trips
the 400ms ceiling unmistakably). The 100ms figure is a LOCAL design goal we
track, not the CI assertion.

The ADVISORY check is a Criterion benchmark (`benches/lsp_didchange.rs`,
`harness = false`) measuring the same round-trip, with a committed reference
baseline (`benches/baseline.json`). Its 20%-regression detection is authoritative
ONLY on a pinned/self-hosted runner; on shared CI it runs `--no-run` (compile-
only, so the harness cannot rot) and any timing it reports is informational
(`continue-on-error: true`). Full timing + regression comparison is a controlled-
runner / local-dev activity.

The benchmark and the budget test share an identical `round_trip` body so the
advisory and the gate measure the same work; both edit the fixture by appending
a `//` comment line (valid syntax — never a bare `#`, which would hang the parser
per 06-07's deferred item) so `set_text` genuinely bumps the revision without
introducing a parse error.

## Consequences

Positive: the CI gate is stable — it never flakes on runner noise yet still
fails on a genuine algorithmic regression, so the team trusts it. The tight
100ms goal is still tracked (Criterion, locally / on a pinned runner) without
being held hostage to shared-CI variance. The canonical fixture now exists and
is reused by the LSP feature integration test and both perf harnesses.

Negative: the hard gate cannot catch a small (e.g. 30%) regression that stays
under the 400ms margin — those are the advisory benchmark's job, which on shared
CI is not blocking. Detecting subtle regressions therefore depends on someone
running the benchmark on a controlled machine (or wiring a self-hosted runner
later). The committed `baseline.json` is a hand-captured reference, not
Criterion's live `target/criterion/` state (which is gitignored + runner-local),
so baseline drift must be refreshed deliberately.

Neutral: measured numbers at authoring time — warm round-trip ≈ 0.12ms
(Criterion slope point estimate), cold/first-edit worst ≈ 3ms (budget test) —
both an order of magnitude under the 100ms goal, so there is comfortable
headroom as the type checker grows.

Alternatives rejected: a naked `< 100ms` hard assert (rejected — flakes on
shared CI, trains the team to ignore the gate); a Criterion-only gate with the
20%-regression check blocking on shared CI (rejected — its variance produces
false positives without a pinned runner); no perf gate at all (rejected — SC#2
needs enforcement, and an unguarded analysis loop silently rots editor latency).
