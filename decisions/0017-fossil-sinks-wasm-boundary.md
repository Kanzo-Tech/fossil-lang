# ADR 0017: `fossil-sinks` depends on `arrow-schema` only and joins the WASM gate as the 7th crate

**Date:** 2026-05-21
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/05-stdlib-sources-graphar-sink-complete/05-RESEARCH.md` §"SC#5 WASM Boundary — RESOLVED", §"Don't Hand-Roll", Pitfall 1; `architecture.md` §6.7; `operator-algebra.md` §2.11/6.7

## Context

`fossil-sinks` is the GraphAr output sink. SINK-06 frames it as "roll our own atop
`arrow` + `parquet` (no Apache GraphAr Rust SDK)", and SC#5 demands that the WASM
dependency boundary stay clean — concretely, `cargo tree -i mio -p fossil-sinks` must
return zero paths.

The single most important realization (RESEARCH §"Don't Hand-Roll") is that **Fossil never
writes Parquet bytes from Rust.** The sink's job is decomposition + SQL generation + manifest
emission; the actual columnar bytes are written by DuckDB's `COPY (...) TO '...' (FORMAT PARQUET)`
on both native (`fossil-runtime` → duckdb) and WASM (DuckDB-WASM). The Rust `arrow`/`parquet`
crates exist only to give the manifest **schema correctness** — column data-type names that match
what DuckDB will write — and are not on the byte-writing path.

This matters because `parquet`'s default `arrow` feature transitively pulls `arrow-ipc`
(needed by `ArrowWriter`), and the project rule (CLAUDE.md hard rules) bans `arrow-ipc`/`arrow-csv`
from the WASM-gated crates as a conservative mio/tokio-leak guard. If `fossil-sinks` were to
byte-write Parquet from Rust, it could not be WASM-clean. Since it does not, it needs at most
`arrow-schema` (the `DataType` enum + names) and a thin native-only `parquet` schema read-back
for tests.

The alternative — keeping decomposition/manifest generation in a native-only crate and excluding
`fossil-sinks` from the WASM gate — would prevent the playground from rendering the manifest +
decomposition SQL in-browser (PLAY-07 "View Compiled SQL" benefits), and would leave the cleanest
boundary unstated.

## Decision

We will make `fossil-sinks` WASM-clean and add it to the WASM gate as the **7th gated crate**:

```
cargo check --target wasm32-unknown-unknown \
    -p fossil-base -p fossil-syntax -p fossil-hir \
    -p fossil-mir -p fossil-codegen -p fossil-sinks -p fossil-wasm
```

`fossil-sinks` depends on `arrow-schema` **only** (the sub-crate; the `DataType` enum +
names; pulls no mio/arrow-ipc/arrow-csv) plus `serde` + `serde_yaml_ng` for manifest emission.
It does NOT depend on the `arrow` umbrella crate, and `parquet` is a **native dev-dependency
with `default-features = false`** (test-only schema read-back; the `default-features = false`
drops parquet's `arrow` feature → `arrow-ipc`). `parquet` is never a normal dependency.

DuckDB COPY — not the Rust `parquet::ArrowWriter` — writes the Parquet bytes. This keeps
`fossil-sinks` WASM-clean and yields identical native↔WASM bytes (SC#2).

The SC#5 audit (run in the WASM gate) is:

```
cargo tree -i mio       -p fossil-sinks --target wasm32-unknown-unknown   # expect: no match
cargo tree -i arrow-ipc -p fossil-sinks --target wasm32-unknown-unknown   # expect: no match
cargo tree -i arrow-csv -p fossil-sinks --target wasm32-unknown-unknown   # expect: no match
```

`arrow`/`parquet` are pinned at `58` (the latest published line as of 2026-05; the prior
aspirational `= "59"` placeholders did not resolve on crates.io and were never exercised because
no compiling crate depended on them until now).

## Consequences

- **Positive:** The playground can render the GraphAr manifest + decomposition SQL in-browser.
  The WASM boundary is explicit and CI-enforced (7 gated crates + the cargo-tree audit). The
  "writer is DuckDB" insight collapses most of SC#5's apparent complexity — `fossil-sinks` is a
  plan + manifest generator, not an I/O library.
- **Negative:** `fossil-sinks` can never grow a Rust byte-writing path without re-litigating this
  ADR. Any future contributor who reaches for `parquet::ArrowWriter` must instead emit a DuckDB
  COPY statement. `parquet` schema read-back tests are native-only (`#[cfg(...)]` or dev-dep).
- **Neutral:** `arrow`/`parquet` pins moved 59 → 58 at the workspace level to match crates.io
  reality; this is the version Cargo.lock already resolved transitively via duckdb. The
  byte-level `arrow`/`parquet` umbrella deps remain available for native test crates.
