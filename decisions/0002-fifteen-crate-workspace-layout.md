# ADR 0002: Adopt the 15-crate workspace layout

**Date:** 2026-05-15
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/research/ARCHITECTURE.md` §Component boundaries; `.planning/phases/00-workspace-genesis-rudof-spike/00-RESEARCH.md` §"Reconciling the crate count"

## Context

Three counts have circulated for the Fossil compiler workspace:

- The original `architecture.md` design corpus listed **17 crates** (one per layer, with `fossil-types`, `fossil-resolve`, `fossil-typeck` as separate crates and a separate `fossil-descriptors-input` / `fossil-descriptors-output` split).
- Project research (`.planning/research/ARCHITECTURE.md`) recommended collapsing the type-system layers into one `fossil-hir` crate, mirroring `ty_python_semantic`'s actual structure. The synthesizer summary stated this brought the count to **13**.
- The math, when checked: 17 crates − 2 collapsed (types + resolve folded into typeck, then typeck renamed to `fossil-hir`) = **15 crates**, not 13. The discrepancy in the synthesizer summary was an off-by-2 error. To actually hit 13 would require collapsing two further pairs (`descriptors-input + descriptors-output` and `ide + ide-db`), each of which has independent reasons against (descriptor split lets each direction grow independent feature surface; ide/ide-db split mirrors rust-analyzer's separation of search infrastructure from feature implementations).

The forces in tension: minimize crate count for Cargo compile-time speed, versus preserve clear API boundaries that resist accidental cross-cutting dependencies. The reference codebases (`ty_python_semantic`, rust-analyzer's `ide-db` + `ide` split) already settled this trade-off; replicating their structure inherits the trade-off without re-deriving it.

## Decision

We will adopt a **15-crate workspace layout**:

```
fossil-base                  Salsa Db trait, VFS, file inputs, ErrorGuaranteed
fossil-syntax                lossless CST (rowan), parser, lexer (logos)
fossil-hir                   types + name resolution + bidirectional checker
fossil-mir                   typed operator algebra IR (11 ops)
fossil-codegen              MIR → DuckDB SQL + GraphAr manifest
fossil-descriptors-input    InputDescriptor trait + impls (CSVW, JSON Schema, XSD, Parquet)
fossil-descriptors-output   OutputDescriptor trait + impls (ShEx via rudof; SHACL fase 2)
fossil-sinks                 Sink trait + GraphAr writer (atop arrow + parquet — no Apache GraphAr Rust SDK exists)
fossil-registry              function registry (FnO + RML-FNML + rmlext:Pipeline)
fossil-runtime              DuckDB native execution (NATIVE-ONLY — bundled C++)
fossil-ide-db               symbol indexes, search infrastructure (rust-analyzer pattern)
fossil-ide                   hover, completion, goto-def, code actions
fossil-cli                   `fossil compile/check/run` (NATIVE-ONLY — clap)
fossil-lsp                   LSP server via lsp-server (NATIVE-ONLY — see ADR-0001)
fossil-wasm                  WASM host shim (FossilPlayground API for browser)
```

WASM CI gate covers the **6 compiler-core crates** (`fossil-base`, `-syntax`, `-hir`, `-mir`, `-codegen`, `-wasm`). The 3 native-only crates (`-runtime`, `-cli`, `-lsp`) carry `compile_error!` cfg-tripwires guarding accidental WASM-CI inclusion.

## Consequences

**Positive:**
- API boundaries are clear: ide-db is reusable by hypothetical Phase 2+ alternative consumers (e.g., a TUI repl); descriptors are pluggable per direction without entangling the input/output stories; sinks are pluggable as a unit.
- Replicates a pattern proven in `ty_python_semantic` (collapsed type+resolve+check) and rust-analyzer (split ide-db / ide). Direct mapping for contributors arriving from those codebases.
- Each crate has a single owner concept — Plan 04's `decisions/` hosts the ADRs, but per-crate decisions can be added without churning a monolith.

**Negative:**
- 2 more `Cargo.toml` files than the synthesizer summary's 13-crate proposal, with the corresponding ~20% workspace metadata overhead. Acceptable.
- Compile time delta: more crate boundaries means more codegen units; the cost is small (Salsa proc-macros dominate either way) but real.

**Neutral:**
- WASM CI gate covers 6 of 15 regardless — adding or removing the descriptor-split changes nothing in the gate set.
- Future split (e.g., when `fossil-hir` crosses ~10k LOC and compile times noticeably drag) is fully reversible: split `fossil-hir` back into `fossil-types` + `fossil-resolve` + `fossil-typeck` with no change to the Db trait. ADR can be superseded by a future ADR-NNNN.
