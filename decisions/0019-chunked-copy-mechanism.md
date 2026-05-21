# ADR 0019: Range-chunked COPY TO PARQUET + a plain-Rust descriptor seam

**Date:** 2026-05-21
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/05-stdlib-sources-graphar-sink-complete/05-RESEARCH.md` (§"Chunked COPY", Open Q1/Q2), ADR-0016 (chunk-file naming), ADR-0017 (DuckDB writes Parquet bytes), ADR-0018 (option-(b) descriptor wiring), CLAUDE.md (no `Box<dyn>` in Salsa; `MAX_PER_MAPPING_FAN_OUT = 1`)

## Context

Plan 05-08 wires the 05-06 `SinkPlan` (ShEx-driven vertex/edge decomposition)
into `fossil-codegen`'s `Op::Sink` arm. Three forces are in tension.

**1. Larger-than-RAM materialization (SINK-03).** GraphAr stores each vertex /
edge table as a sequence of fixed-size Parquet *chunks* (`<prefix>chunk{k}.parquet`,
ADR-0016). A single `COPY (...) TO 'output.parquet'` cannot express that layout,
and a table whose row count exceeds `chunk_size` must be split into N > 1 files.
DuckDB offers two mechanisms: (a) **range-chunked multi-COPY** — N separate
`COPY` statements, each selecting one `row_number()` window
`WHERE _rn BETWEEN k*chunk_size+1 AND (k+1)*chunk_size`; or (b) DuckDB's native
`COPY ... TO 'dir' (FORMAT PARQUET, PARTITION_BY ...)` that the engine expands to
N files. We must pick one and prove multi-chunk emission.

**2. The descriptor must not enter a Salsa key (B2).** The decomposition needs
the target `OutputDescriptorKind`, which carries a `ShExDescriptor` (a
`shex_ast::Schema` + a resolved `HashMap<String, ShapeBinding>`). That heap state
does NOT satisfy the `salsa::Update` / `Eq` / `Hash` / `Clone` bounds Salsa
interning requires. Threading it into a `#[salsa::tracked]` key is impossible
*and* would add a per-mapping descriptor read, breaking
`MAX_PER_MAPPING_FAN_OUT = 1` (RESEARCH Pitfall 6). Yet the existing
`#[salsa::tracked] codegen_graph(db, mir)` must stay tracked for the
type-checked / interned MIR work.

**3. Native↔WASM parity (Pitfall 5).** The chunk boundaries must be reproducible
across engines so the SC#1 parity digest is stable: chunk k must contain the same
rows on native DuckDB and DuckDB-WASM.

## Decision

**We will use range-chunked multi-COPY (mechanism a), emitted by a plain-Rust
outer wrapper over the tracked `codegen_graph`.**

**Chunking mechanism.** For each decomposed table the inner deterministic-`ORDER
BY` SELECT (from 05-06's `vertex_select_sql` / `edge_select_sql`) is wrapped with
`row_number() OVER (ORDER BY <id>) AS _rn`, and **one** hand-formatted
`COPY (SELECT * EXCLUDE (_rn) FROM (<numbered>) WHERE _rn BETWEEN k*chunk_size+1
AND (k+1)*chunk_size) TO '<prefix>/chunk{k}.parquet' (FORMAT PARQUET)` statement
is emitted per chunk `k in 0..ceil(row_count / chunk_size)`. We chose range-COPY
over DuckDB's `PARTITION_BY` because (i) it gives byte-stable, named per-chunk
files matching the ADR-0016 `chunk{k}.parquet` convention without relying on
DuckDB's hive-partition directory naming, (ii) the chunk count is explicit in the
emitted SQL (auditable / snapshot-testable), and (iii) the `row_number()` window
over the mandatory deterministic `ORDER BY` makes chunk boundaries reproducible
across native↔WASM (force 3). The COPY wrapper stays **hand-formatted** — never
round-tripped through sqlparser (Pitfall 1).

**Chunk filename convention.** `<prefix>/chunk{k}.parquet` where `prefix` is
`vertex/<type>/` or `edge/<src>_<pred>_<dst>/` (lowercased), matching the manifest
`prefix` field (ADR-0016, RESEARCH Open Q1).

**chunk_size config surface (RESEARCH Open Q2).** A `chunk_size: u64` parameter is
threaded through the codegen seam; the default is
`fossil_sinks::manifest::DEFAULT_CHUNK_SIZE` (1024), re-exported as
`fossil_codegen::SINK_DEFAULT_CHUNK_SIZE`. A Phase-6 CLI `--chunk-size` flag
overrides it without touching codegen.

**Row-count oracle.** The chunk count needs the table's row count. Because chunk
emission is NOT a tracked query (it is a plain-Rust post-pass), `codegen_sql_with_descriptor`
takes a `row_count_for: FnMut(&str) -> Option<u64>` closure — the runtime supplies
the count via a `SELECT count(*)` over the inner SELECT (legitimate here; no Salsa
involvement). `None` (unknown / descriptor-less) emits a single chunk.

**The plain-Rust descriptor seam (B2 resolution).**
`codegen_sql_with_descriptor(db, mir, &OutputDescriptorKind, chunk_size,
row_count_for)` is a **plain-Rust function, NOT `#[salsa::tracked]`**. It:
1. calls the existing `#[salsa::tracked] codegen_graph(db, mir)` for the
   type-checked / interned MIR work (unchanged — the only tracked query; the
   descriptor never enters its key);
2. applies the descriptor-driven vertex/edge decomposition + chunked COPY
   emission as a plain-Rust post-pass over the MIR `Op`s + the descriptor.

`AcceptAll` (no ShEx target — the walking-skeleton) returns `codegen_graph`'s
flat-triple single COPY **byte-identically**. So `OutputDescriptorKind` stays out
of every Salsa key, no interning bounds are needed, and
`MAX_PER_MAPPING_FAN_OUT = 1` is structurally preserved.

**WASM resolution.** Adding `fossil-sinks` (→ `fossil-descriptors-output` →
`shex_ast`) to `fossil-codegen` extends the 7-crate WASM-gated chain. A fail-fast
`cargo check --target wasm32-unknown-unknown -p fossil-codegen` run immediately
after adding the dep confirmed the chain is WASM-clean — **no cfg-fence required**
(ADR-0018 already established `fossil-descriptors-output` is WASM-clean; that
resolution inherits transitively through codegen).

**Manifest collapse.** The duplicated Phase-1 `manifest_template()` constant in
`fossil-codegen/src/manifest.rs` is collapsed: it now delegates to
`fossil_sinks::GraphArSink::manifest_template` (one source of truth, byte-identical
so the `compile_hello` snapshot stays green). The descriptor path emits the
programmatic GraphAr v1.0.0 manifest via `manifest_yaml_for_plan`
(→ `fossil_sinks` `Sink::manifest_for`).

## Consequences

**Easier.** GraphAr-conformant chunk layouts are now emitted directly; a table
larger than `chunk_size` splits into N named Parquet files with a stable,
auditable boundary. The descriptor seam is testable without a `.fossil` source
(option (b) fixture). Phase-6 Db-wiring lights up the same plain-Rust seam by
supplying the resolved descriptor + a real `row_count_for`, with zero codegen
changes. The manifest constant can no longer drift between crates.

**Harder.** The emitted SQL is more verbose (one COPY per chunk + the
`row_number()` window subquery). The `row_count_for` oracle requires a runtime
`count(*)` round-trip per table before the COPYs run — acceptable, since it is a
single cheap aggregate and keeps the chunk count exact.

**Risks.** A `PARTITION_BY`-based future optimization (fewer round-trips, engine
parallelism) would change the on-disk file names; revisit if range-COPY round-trip
cost becomes a bottleneck. The plain-Rust seam means the descriptor post-pass is
NOT memoized by Salsa — re-running codegen for an unchanged mapping + descriptor
recomputes the COPY text. This is acceptable: the post-pass is cheap string
assembly over an already-memoized `codegen_graph` result.
