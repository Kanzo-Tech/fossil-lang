# Architecture Decision Records (ADRs)

This directory contains the ADRs for the Fossil project, using the
[Nygard format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).

## When to write an ADR

Per `CLAUDE.md` and `CONTRIBUTING.md`: any choice between alternatives that took
**more than 15 minutes to decide** gets an ADR within 24 hours of the decision.

ADRs are how a solo project survives the bus-factor-1 problem. They are also
how Future-Angel reads Past-Angel's reasoning without ambient context.

## File naming

`NNNN-verb-noun-phrase.md` (4-digit zero-padded, lowercase, dash-separated).

Examples:
- `0001-use-lsp-server-not-tower-lsp.md`
- `0002-fifteen-crate-workspace-layout.md`
- `0003-thin-db-trait-with-system-abstraction.md`

Special non-numbered files (decision logs that aren't ADRs proper):
- `rudof-wasm.md` — Phase 0 spike outcome (decision-shaped but tied to a one-off experiment)
- `rudof-wasm-email-draft.md` — outbound communication record

## Template

Use [`template.md`](template.md) as the starting point. Copy, fill in, commit.

## Status lifecycle

- `proposed` — drafted but not yet committed to
- `accepted` — in force
- `deprecated` — no longer recommended; not yet replaced
- `superseded by ADR-NNNN` — replaced by a later decision

## Index

| # | Title | Status | Date |
|---|-------|--------|------|
| 0001 | Use lsp-server, not tower-lsp | accepted | 2026-05-15 |
| 0002 | Adopt the 15-crate workspace layout | accepted | 2026-05-15 |
| 0003 | Db trait is thin; descriptors and registry live behind System | accepted | 2026-05-15 |
| 0004 | Workspace lint `unsafe_code = "deny"`, not `"forbid"` | accepted | 2026-05-15 |
| [0005](0005-item-tree-body-separation.md) | ItemTree carries signatures only; bodies live behind per-mapping `body()` query | accepted | 2026-05-18 |
| [0006](0006-output-descriptor-kind-enum-dispatch.md) | `OutputDescriptorKind` enum dispatch + `SystemWithDescriptors` extension trait + structured `Diagnostic.suggestion_source` | accepted | 2026-05-19 |
| [0007](0007-csvw-jsonld-subset-cutoff.md) | Cap CSVW Metadata Vocabulary support at a literal-`@context` JSON-LD subset | accepted | 2026-05-19 |
| [0008](0008-real-spans-via-side-table.md) | Real per-mapping spans via a `Spans<'db>` side table (NOT a field on `HirExpr`) | accepted | 2026-05-19 |
| [0009](0009-mir-reachability-direct-construction.md) | Define all 11 MIR ops; lower 4 from source, test 7 via direct construction; Op::Empty for R9; defer surface pipeline syntax | accepted | 2026-05-20 |

(ADRs 0001-0003 are written in Plan 05; this README is updated as new ADRs land.)

## Spike outcomes (non-ADR)

| File | Topic | Outcome |
|------|-------|---------|
| `rudof-wasm.md` | rudof crates on wasm32-unknown-unknown | See file |

## Sources

- [Nygard 2011 — Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
- [Joel Parker Henderson — ADR repository](https://github.com/joelparkerhenderson/architecture-decision-record)
