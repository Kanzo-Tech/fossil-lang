# ADR 0003: Db trait is thin; descriptors and registry live behind System

**Date:** 2026-05-15
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/research/ARCHITECTURE.md` §Pattern 1 — Salsa db trait composition; `ruff_db` source (https://github.com/astral-sh/ruff/blob/main/crates/ruff_db/src/lib.rs)

## Context

The original `architecture.md` design corpus proposed composing the Salsa `Db` trait from three host capability traits:

```rust
#[salsa::db]
pub trait Db: HasInputDescriptors
              + HasOutputDescriptors
              + HasRegistry
              + salsa::Database {}
```

Each `Has*` trait carried one or more `&dyn` accessors for that capability. The host (CLI, LSP, WASM) implemented all three.

Project research (`.planning/research/ARCHITECTURE.md`) examined `ruff_db`'s actual `Db` trait and found a markedly thinner shape — a single trait with two narrow methods:

```rust
#[salsa::db]
pub trait Db: salsa::Database {
    fn system(&self) -> &dyn System;
    fn files(&self) -> &Files;
}
```

Descriptors, the registry, and host-injected capabilities (filesystem, network for native, none for WASM) live behind the single `dyn System` indirection. `ty_wasm` composes `Workspace { db, system }` and the WASM host implements `System` once.

The forces in tension: granular trait composition (architecture.md's proposed shape) gives stronger compile-time decoupling and fine-grained dyn-dispatch costs, while a thin Db with a fat System gives one swap point for the entire host abstraction (WASM vs native), one vtable per query call, and is the verified pattern in two reference codebases (ruff_db, ty_wasm).

## Decision

We will adopt the **thin Db trait with `System` abstraction**:

```rust
// crates/fossil-base/src/db.rs

#[salsa::db]
pub trait Db: salsa::Database {
    fn system(&self) -> &dyn System;
    fn files(&self) -> &Files;
}

pub trait System: Send + Sync {
    // Filesystem (abstract — VirtualFS for WASM, std::fs for native)
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, FsError>;

    // Descriptor registries
    fn input_descriptor(&self, kind: &str) -> Option<&dyn InputDescriptor>;
    fn output_descriptor(&self, kind: &str) -> Option<&dyn OutputDescriptor>;

    // Function registry
    fn registry(&self) -> &FunctionRegistry;

    // Time / random — needed for stdlib parse/datetime + anon/hash
    fn now(&self) -> SystemTime;
    fn random_seed(&self) -> u64;
}
```

The host crates implement `System` once: `WasmSystem` (in `fossil-wasm`), `NativeSystem` (in `fossil-runtime` + `fossil-cli` + `fossil-lsp`).

## Consequences

**Positive:**
- One swap point: switching native ↔ WASM means swapping the `System` impl, not three trait impls. `WasmSystem` lives in `fossil-wasm`, `NativeSystem` lives in shared code consumed by CLI/LSP/runtime.
- Minimal vtable cost: every Salsa query that needs descriptors does one vtable hop (`db.system()`) then narrows from there. Hot paths can cache `&dyn System` once and reuse.
- Verified pattern in ruff_db + ty_wasm — proven scalable and compatible with Salsa 0.26's interned + tracked types.
- Hot-swappable for testing: `MockSystem` is one impl, not three; integration tests inject it without reaching for type erasure tricks.
- Aligns with the rudof spike fallback design from `decisions/rudof-wasm.md`: even though path (a) means we wire rudof directly, the `OutputDescriptor` trait under `System` keeps the fallback option plug-in-able for free.

**Negative:**
- Two-hop access for descriptors at call sites (`db.system().output_descriptor("shex")` instead of `db.output_descriptor("shex")`). Verbose at call sites until the helper functions wrap it.
- One large `System` trait instead of many small ones — refactoring `System` (e.g., adding a `metrics()` method) is a workspace-wide rebuild even though no individual crate depends on it semantically.

**Neutral:**
- Consistent with the `OutputDescriptor` plug-in fallback design committed to in `decisions/rudof-wasm.md` — the trait abstraction stays as insurance for v2 SHACL Core add-on even when the v0.1 path is direct rudof integration.
- Future split: if `System` grows beyond ~10 methods, splitting into `FileSystem`, `Registry`, `Clock` etc. is mechanical (each becomes a separate `&dyn` in `System` itself, not a separate trait on `Db`). ADR can be superseded.
