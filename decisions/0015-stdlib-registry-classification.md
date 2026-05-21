# ADR 0015: Classify stdlib functions in a static registry with a PureSql ⟺ non-Udf invariant

**Date:** 2026-05-21
**Status:** accepted
**Decider:** Ángel Iglesias
**Cite:** `.planning/phases/05-stdlib-sources-graphar-sink-complete/05-RESEARCH.md` (Pattern 1, §"Stdlib Registry"); `stdlib.md` (the authoritative function catalog); `type-system.md` (FnSig); ADR-0003 (Db trait shape), ADR-0006 (enum-dispatch over Box<dyn>), ADR-0009 (MIR reachability / direct construction)

## Context

Phase 5 must give every Fossil v0.1 standard-library function a single source of
truth: its signature, how it lowers to DuckDB SQL, and whether that lowering runs
in the browser playground (pure SQL) or requires a native Rust UDF (native-only).
`fossil-codegen`'s `render_expr` will consult this for `Expr::Call` lowering
(plan 05-02); the UDF registration manifest (05-03) and the SC#1 CI classification
gate (05-07) both enumerate it. The Phase-1 registry carried only four entries and
a coarse four-variant `RegistryKind` tag — insufficient.

Three forces are in tension:

1. **No `Box<dyn Trait>` may cross a Salsa boundary** (CLAUDE.md hard rule,
   ADR-0003/ADR-0006). `render_expr` runs inside the `#[salsa::tracked]`
   `codegen_graph` query, so the registry it reads must use concrete owned types
   and enum dispatch, never a trait object fetched through `db.system()`.

2. **The registry should be program-invariant and cheaply shareable.** The stdlib
   is fixed in v0.1 (no federation — REG-01 deferred), so a `&'static`
   `FunctionRegistry` behind a `LazyLock`/`OnceLock` at the consumer is the natural
   shape and sidesteps the Salsa-interning hazard entirely.

3. **Signatures want to be real `fossil_hir::FnSig`s**, not a bespoke string-sig
   parser (which would add surface area and a parse-failure path). But `FnSig<'db>`
   is `#[salsa::interned]`: it carries a `'db` lifetime and needs a database to
   construct, so it cannot live inside a `'static` value. Forces 2 and 3 collide.

We also need the classification (pure-SQL vs native-UDF) to be impossible to get
internally inconsistent: a function tagged `PureSql` but lowered through a Rust UDF
would silently break the playground (SC#1).

Finally, the catalog must match `stdlib.md` — the authoritative spec per CLAUDE.md
— **exactly and bidirectionally**: no omissions and no extras. `stdlib.md`'s `math/`
defines exactly six functions (`sum`, `avg`, `min`, `max`, `abs`, `round` — no
`ceil`/`floor`); `anon/` includes `redact`; `validate/` includes `regex`.

## Decision

We will model each stdlib function as a concrete
`RegistryEntry { name, sig: SigSpec, lowering: LoweringKind, wasm_class: WasmClass }`
held in an owned `FunctionRegistry` built once by `FunctionRegistry::stdlib_default()`
and shared as a `&'static` at the consumer. Dispatch is by enum, never `Box<dyn>`:

- `LoweringKind` ∈ `{ Builtin{duckdb_name}, Inline(InlineForm), Udf{udf_name}, Plan(PlanOp) }`.
- `WasmClass` ∈ `{ PureSql, NativeUdfOnly }`.

We will enforce the invariant **`wasm_class == NativeUdfOnly` iff `lowering` is
`Udf` (equivalently `lowering ∈ {Builtin, Inline, Plan}` ⟺ `PureSql`)** mechanically:
every catalog insertion sets `wasm_class` through a single private
`derive_wasm_class(&LoweringKind) -> WasmClass` helper, so the two fields cannot
drift. A unit test asserts the invariant for every entry; the SC#1 CI gate (05-07)
reuses the same derivation plus a curated DuckDB-builtin allowlist.

We will resolve the `FnSig`-vs-`'static` collision by storing a `'db`-free
`SigSpec { params: Vec<ScalarTy>, ret: ScalarTy }` in the static registry, where
`ScalarTy` is a small `'db`-free tag mirroring the scalar subset of
`fossil_hir::TyKind`. `RegistryEntry::signature(db)` interns a real `FnSig<'db>`
on demand by constructing each `Ty<'db>` directly via the fossil-hir constructor
(`Ty::new` / `FnSig::new`). This honours both "construct `FnSig` directly via the
fossil-hir constructor" and "the registry is `&'static`" — no string-sig parser.

We will populate `stdlib_default()` to equal the eight surface namespaces of
`stdlib.md` exactly (56 functions: core 8, seq 13, clean 6, parse 7, math 6, str 8,
validate 5, anon 3), guarded by a **bidirectional** set-equality test that fails on
both omissions and extras. The `seq/` family and the `io/` source constructors are
`LoweringKind::Plan` and are surface-unreachable in v0.1 (ADR-0009 cross-ref):
their MIR ops are built and codegen-verified via direct `MirGraph` construction,
but no `.fossil` source can invoke them yet, so STDL-01 is discharged by registry
entries + the existing direct-MIR tests, not a `seq.filter(...)` example. The three
`io/` constructors (`io.csv`/`io.json`/`io.parquet`) are registered for the
walking-skeleton and 05-04 source lowering but are NOT part of the eight-namespace
completeness set; `io/sql` and `io/http` are out of scope this milestone.

We explicitly reconcile to `stdlib.md`: `math/` has exactly six functions (no
`ceil`/`floor`); `anon.redact` is `Inline(InlineForm::LiteralStr{"[REDACTED]"})`
and `validate.regex` is `Builtin{regexp_matches}` — both `PureSql`.

## Consequences

**Easier.** Codegen, UDF registration, and the SC#1 CI gate all read one typed,
program-invariant table with no trait objects and no Salsa hazard. The classification
cannot become internally inconsistent (single derivation point). The catalog cannot
silently drift from `stdlib.md` (bidirectional completeness test). Signatures are
real interned `FnSig`s when the checker needs them, with zero string-parsing surface.

**Harder / accepted limitations.** Signatures in `SigSpec` are v0.1 scalar
approximations: the surface syntax for pipelines, higher-order `Fn(...)` arguments,
`forall`-polymorphism, and schema-directed `parse.json` does not exist yet
(RESEARCH Open Q4), so generic/higher-order positions collapse to their dominant
scalar shape (predicate-taking `seq/` ops typed `(String) -> String`; `parse.json`
returns `String`). These are documented per-entry where they deviate from the ideal
and must be revisited when surface pipeline syntax lands. `SourceFormatTag` is a
registry-local mirror of `fossil-mir::SourceFormat` (to avoid a dependency cycle);
plan 05-04 (STDL-06) reconciles the two.

**New risk.** A future federated/user-defined function registry (REG-01) would break
the `&'static` assumption; that is explicitly deferred past v0.1, and revisiting it
should produce a follow-up ADR. The `Builtin.duckdb_name` allowlist (the second half
of the SC#1 gate) is owned by 05-07; until then, only non-emptiness is checked here.
