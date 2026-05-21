# ADR 0016: GraphAr v1.0.0 manifest schema via `serde_yaml_ng` structs

**Date:** 2026-05-21
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/05-stdlib-sources-graphar-sink-complete/05-RESEARCH.md` §"GraphAr Sink", Pattern 3, Pitfall 2, Open Question 1; [GraphAr v1.0.0 format spec](https://graphar.apache.org/docs/specification/format)

## Context

The Phase-1 GraphAr sink emitted a hand-templated YAML constant — duplicated in both
`fossil-sinks/src/lib.rs` and `fossil-codegen/src/manifest.rs` — using the field spelling
`graphar_version: 1.0.0`, `vertex_types:`, `data_type: string`, `nullable:`, `parquet_path:`.
That spelling was explicitly "skeletal / aspirational"; it conforms to **no** GraphAr reader.

The GraphAr v1.0.0 format spec uses different field names: a vertex-info file carries
`version: gar/v1`, `type:` (the vertex label), `chunk_size`, `prefix`, and `property_groups`
(each a `file_type` + a list of properties with `name`/`data_type`/`is_primary`/`is_nullable`).
An edge-info file carries `src_type`/`edge_type`/`dst_type`, `chunk_size` + `src_chunk_size` +
`dst_chunk_size`, `directed`, `prefix`, `adj_lists` (each `ordered`/`aligned_by`/`file_type`),
and `property_groups`. SINK-02 requires generating this **programmatically** (not by string
templating) with `serde_yaml_ng` (NOT `serde_yml` — RUSTSEC-2025-0068).

Two sub-questions:

1. **`data_type` spellings.** The manifest's declared column types must match what DuckDB COPY
   actually writes. The authority for the spec spellings (`int64`, `string`, `double`, `bool`, ...)
   is `arrow_schema::DataType` (RESEARCH §"Don't Hand-Roll"), not a hand-maintained match.
2. **Chunk-file naming convention (RESEARCH Open Q1).** GraphAr stores fixed-row chunks under the
   vertex/edge `prefix`. The exact per-chunk file name is a convention the writer and reader must
   agree on; the spec leaves room (`chunk0`, `part-0`, etc.).

## Decision

We will replace the hand-templated constant with `#[derive(Serialize)]` structs in
`fossil-sinks/src/manifest.rs` — `VertexInfo`, `EdgeInfo`, `PropertyGroup`, `Property`, `AdjList`
— serialized via `serde_yaml_ng::to_string`. The structs use the exact GraphAr v1.0.0 field names
via serde renames (`vertex_type` → `type`; `version: gar/v1` preset via `GRAPHAR_VERSION`;
`is_nullable` carries `#[serde(skip_serializing_if = "Option::is_none")]`).

`data_type` strings are produced by `data_type_name(&arrow_schema::DataType) -> String`, the single
authority for the spec spellings; unhandled arrow types fall back to `binary`.

The chunk-file naming convention is **`<prefix>chunk{k}.parquet`** (e.g.
`vertex/person/chunk0.parquet`), zero-indexed, one Parquet file per fixed-row chunk per property
group. The manifest declares `chunk_size` (default `1024`, configurable per mapping); the runtime
materializes the chunks (the chunked COPY emission lands in plan 05-08, and a chunk read-back via
DuckDB is verified there). These structs are plain serializable data — no `Box<dyn Trait>` — so
they pass safely through Salsa queries.

This supersedes the Phase-1 `graphar_version:`/`vertex_types:` template. `fossil-sinks::Sink::manifest_template`
is retained additively (per the Phase-1 trait contract) but documented as superseded; the
codegen-side duplicate `fossil_codegen::manifest::manifest_template` is collapsed onto these
structs in plan 05-08.

A `tests/manifest_yaml.rs` `insta` snapshot freezes the serialized YAML shape, and unit-test
field-name guards assert the spec spellings are present and the Phase-1 spellings are absent
(Pitfall 2 guard).

## Consequences

- **Positive:** The emitted manifest is accepted by GraphAr v1.0.0 readers (GraphScope etc.).
  Manifest generation is programmatic and type-driven; `data_type` spellings track `arrow-schema`
  rather than drifting in a hand-maintained match. The snapshot + field-name guards catch any
  regression to the Phase-1 spelling.
- **Negative:** Two manifest spellings coexist transiently until plan 05-08 collapses the
  codegen-side duplicate; readers must use the new structs, not `manifest_template()`. The
  `<prefix>chunk{k}.parquet` convention is now load-bearing — the 05-08 chunked COPY and any
  reader must honor it (verified by a DuckDB read-back in 05-08).
- **Neutral:** `chunk_size` is declared in the manifest but the actual chunk materialization is a
  runtime concern deferred to 05-08; this ADR fixes only the schema + naming convention.
