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
| [0019](0019-chunked-copy-mechanism.md) | Range-chunked multi-COPY (one `COPY ... TO '<prefix>/chunk{k}.parquet'` per `row_number()` window `WHERE _rn BETWEEN k*chunk_size+1 AND (k+1)*chunk_size`) over DuckDB `PARTITION_BY`, for stable named chunks + auditable count + native↔WASM parity; `chunk_size` default 1024 + threaded param; the descriptor seam `codegen_sql_with_descriptor` is a PLAIN-RUST outer wrapper over the tracked `codegen_graph` (`OutputDescriptorKind` never enters a Salsa key — fan-out=1 preserved); manifest-template dup collapsed to `fossil_sinks` | accepted | 2026-05-21 |
| [0020](0020-db-descriptor-accessor-wiring.md) | Thread the output descriptor through a `fossil-hir`-owned `HirDb: fossil_base::Db` extension accessor (R2), NOT `fossil_base::Db` (R1 — would invert the dep direction, violating ADR-0006); `resolve_target_shape(db, mapping, kind: &OutputDescriptorKind)` reads it as a PLAIN argument (ADR-0018 "argument, not key": never interned, never a Salsa key, no `Box<dyn>` — fan-out=1 preserved), so it now returns `Some` in production for a host `ShEx` schema (resolves Phase-3 deferral #3 + #8; AcceptAll → None degraded fallback retained); Wave-0 spikes: lsp-types 0.97 wasm32 GREEN (fossil-ide may return lsp_types), R2 cycle-free | accepted | 2026-05-21 |
| [0021](0021-didchange-benchmark-ci-gate.md) | Split SC#2 enforcement: a MARGINED wall-clock correctness test (`tests/didchange_budget.rs`, 400ms budget vs. the 100ms design goal) is the HARD CI gate — catches algorithmic blow-up in the `didChange` round-trip (`set_text`→`def_map`→`typecheck`→accumulator drain over `canonical_200.fossil`) without flaking on shared-runner noise; the Criterion benchmark (`benches/lsp_didchange.rs` + committed `baseline.json`) is ADVISORY (its 20%-regression check is reliable only on a pinned/self-hosted runner; `--no-run` compile-only + `continue-on-error` on shared CI). A naked `<100ms` assert rejected (flakes); Criterion-only blocking gate rejected (variance). Measured: warm ≈0.12ms, cold-edit worst ≈3ms — both an order of magnitude under goal | accepted | 2026-05-22 |
| [0022](0022-salsa-cancellation-model.md) | Use Salsa 0.26's revision-based cooperative cancellation (NO `db.cancel_pending()` — it does not exist); production trigger is a `Setter` mutation (`set_text`) per `didChange` that bumps the revision + sets the shared cancellation flag; `unwind_if_revision_cancelled` is the cooperative checkpoint (auto at query boundaries); analysis runs in `Cancelled::catch`. SC#5 verified at the Salsa unit level with a deterministic two-`Barrier` test (no sleeps) using `token.cancel()` (per-handle token, non-blocking) — asserts the in-flight query unwinds AND post-checkpoint work never ran. Decoupled from the threaded-vs-lazy loop model (finalised in 06-08); custom `AtomicBool` flag rejected (fights Salsa) | accepted | 2026-05-22 |
| [0023](0023-multi-file-workspace-symbol-model.md) | Model the multi-file workspace as the set of OPEN files (LSP `LspState.files` / playground panels) — `WorkspaceIndex::build(db, files: &[SourceFile])` aggregates per-file `SymbolIndex`es — NOT a filesystem scan of `workspace_folders` (heavier, needs FS access, conflicts with the WASM VirtualFS; deferred to v2). Plain structs, no Salsa query, no `Box<dyn>`, fan-out=1 preserved; cross-file goto-def + auto-import (SC#4, LSP-01) work over the open set; closing a file removes its symbols | accepted | 2026-05-22 |
| [0024](0024-fossil-wasm-workspace-api.md) | `fossil-wasm` IS the LSP server-side in the browser (NOT a recompiled `fossil-lsp` — `lsp-server` + stdio aren't WASM-portable; ADR-0001's `compile_error!` cfg-tripwire stays non-negotiable). Grows `FossilPlayground` with the `ty_wasm`-shaped Workspace lifecycle (`open_file` / `update_file` / `close_file` / `check` / `diagnostics_for` / `compile_file` / `set_target_shex`); `FileHandle` is a `u32` newtype, internal map is `HashMap<FileHandle, SourceFile>` (no Salsa key, no `Box<dyn>`). `update_file` uses Salsa `Setter` (`set_text`) — same mechanism as Phase 6 `didChange` (ADR-0022); `MAX_PER_MAPPING_FAN_OUT=1` preserved. One crate, two hosts (fossil-lsp native stdio + fossil-wasm WASM postMessage) — validates the EXT-01 vision | accepted | 2026-05-23 |

(ADRs 0001-0003 are written in Plan 05; this README is updated as new ADRs land.
ADR-0021 landed with Phase-6 plan 06-09, as reserved.)

## Spike outcomes (non-ADR)

| File | Topic | Outcome |
|------|-------|---------|
| `rudof-wasm.md` | rudof crates on wasm32-unknown-unknown | See file |

## Sources

- [Nygard 2011 — Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
- [Joel Parker Henderson — ADR repository](https://github.com/joelparkerhenderson/architecture-decision-record)
