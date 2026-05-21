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
| [0010](0010-rewriting-engine-plain-rust-fixpoint.md) | MIR rewriting is a plain-Rust fixpoint (R1–R10), NOT a Salsa-tracked query — inlined into `lower_to_mir` so it adds zero tracked queries; canonical push + idempotent rules + iteration cap guarantee termination | accepted | 2026-05-20 |
| [0011](0011-r9-empty-source-representation.md) | R9 rewrites a statically-false filter to a dedicated `Op::Empty { schema }` (not a `Source` with a marker); codegen emits `SELECT … WHERE false` / `LIMIT 0` preserving the declared schema | accepted | 2026-05-20 |
| [0012](0012-codegen-sqlparser-ast-hybrid.md) | Build codegen SELECT bodies via sqlparser 0.59 AST construction (structured FROM/WHERE/projection/DISTINCT/UNION/EXCLUDE), hand-wrap the COPY statement (Pitfall 1), render leaf expr fragments via the string path | accepted | 2026-05-20 |
| [0013](0013-mir-type-preservation.md) | MIR types are operationally erasable — codegen ignores them; `codegen(g) == codegen(erase_types(g))` over the 30-mapping corpus (SC#3 type preservation, induction over the DAG) | accepted | 2026-05-20 |
| [0014](0014-sql-parity-two-tier.md) | SC#1 SQL parity is two-tier: automated native `duckdb` execution writes `native_baseline.json`; a documented manual DuckDB-WASM node harness reproduces + diffs the digests at phase close | accepted | 2026-05-20 |
| [0015](0015-stdlib-registry-classification.md) | Classify the stdlib in a `&'static` `FunctionRegistry` (enum dispatch, no Box<dyn> in Salsa); `RegistryEntry{name, SigSpec, LoweringKind, WasmClass}`; the `PureSql ⟺ lowering ∈ {Builtin,Inline,Plan}` invariant via a single `derive_wasm_class` helper; catalog reconciled to `stdlib.md` exactly (bidirectional) | accepted | 2026-05-21 |
| [0016](0016-graphar-manifest-schema.md) | GraphAr v1.0.0 manifest via `serde_yaml_ng` structs (`VertexInfo`/`EdgeInfo`/`PropertyGroup`/`Property`/`AdjList`; `version: gar/v1`, `type`, `src_type`/`dst_type`/`adj_lists`); `data_type` spellings from `arrow_schema::DataType`; chunk-file naming `<prefix>chunk{k}.parquet`; supersedes the Phase-1 `graphar_version:`/`vertex_types:` template | accepted | 2026-05-21 |
| [0017](0017-fossil-sinks-wasm-boundary.md) | `fossil-sinks` depends on `arrow-schema` only (DuckDB COPY writes Parquet bytes, not Rust `ArrowWriter`); `parquet` is a native dev-dep with `default-features=false`; `fossil-sinks` joins the WASM gate as the 7th gated crate; SC#5 cargo-tree audit (mio/arrow-ipc/arrow-csv) | accepted | 2026-05-21 |
| [0018](0018-shex-decomposition-wiring.md) | ShEx-driven sink decomposition is driven by an `OutputDescriptorKind` argument (SC#4 option b), NOT `Db::system()`; full Db-wiring stays Phase 6 (lights up the same seam, zero decomp changes); honest discharge mode "sink-path real files via a fixture ShEx"; no WASM cfg-fence needed (`fossil-descriptors-output` is WASM-clean, gate stays green) | accepted | 2026-05-21 |

(ADRs 0001-0003 are written in Plan 05; this README is updated as new ADRs land.)

## Spike outcomes (non-ADR)

| File | Topic | Outcome |
|------|-------|---------|
| `rudof-wasm.md` | rudof crates on wasm32-unknown-unknown | See file |

## Sources

- [Nygard 2011 — Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
- [Joel Parker Henderson — ADR repository](https://github.com/joelparkerhenderson/architecture-decision-record)
