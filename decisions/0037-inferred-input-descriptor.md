# ADR 0037: Drop user-facing CSVW; infer input schema via host-side DuckDB DESCRIBE; introduce InferredDescriptor

**Date:** 2026-05-26
**Status:** accepted
**Decider:** Ángel Iglesias
**Cite:** `.planning/phases/13-input-model-simplification/13-CONTEXT.md` §"Estrategia: deprecate user-facing, internal CSVW IR opcional"; ADR-0007 §"What's IN the v0.1 subset"; `crates/fossil-hir/src/infer.rs` doc comment about MAX_PER_MAPPING_FAN_OUT.

## Context

v0.1 shipped a model where the user wrote a CSVW JSON-LD descriptor sidecar and
the type-checker performed forward propagation by parsing that descriptor (Phase 3
CORE-05, ADR-0007). In practice, this broke the v0.1 product promise of
"10 seconds to a triple": before the user could see any output, they had to
understand CSVW's JSON-LD subset, hand-roll a descriptor, and keep it in sync
with the source file. The playground CSVW panel (Phase 9 plan 09-07) papered
over the syntactic burden but did not erase the conceptual one — users still
had to think about a separate metadata document to type-check a single mapping.

Two facts changed that make a different model viable for v0.2:

1. The playground ships DuckDB-WASM (Phase 7, ADR-0025): the browser can run
   `DESCRIBE read_csv_auto('<resolved-url>')` against the same file the mapping
   references, producing a column-name + column-type list that is good enough
   to type-check forward propagation.
2. The native CLI links DuckDB natively (CLAUDE.md stack pin `duckdb = "1.10502"`,
   `features = ["bundled"]`): `fossil compile examples/hello.fossil` can perform
   the same introspection transparently before typecheck.

The architectural constraint is layer separation: the Rust compiler MUST NOT
initiate network IO (it would break the WASM gate's portability invariant — the
9 gated WASM crates must build for `wasm32-unknown-unknown` without leaking
network deps — and would also break Salsa determinism, since network responses
are nondeterministic with respect to Salsa's revision model). The introspection
therefore happens outside the Rust compiler crates: in the browser host
(DuckDB-WASM) or in fossil-cli (native DuckDB, NOT in the WASM-gated set).

The forces in tension are:
- **Promise**: "10s to a triple" demands the user write only `io.csv("path")`,
  with no sidecar metadata document.
- **Determinism**: Salsa requires that compiler queries depend only on tracked
  inputs (per ADR-0003's System trait); network IO inside a Salsa query is a
  hard no.
- **Layer separation (WASM gate)**: the WASM-gated compiler crate set must not
  bring in network / async / IO dependencies; CLAUDE.md hard rule "no `tokio`
  outside fossil-lsp" reinforces this.
- **Backwards compatibility**: v0.1 `.fossil` files in the wild use the explicit
  `schema = "..."` descriptor argument; v0.1 permalinks include a `csvw` field;
  both should still load in v0.2.
- **CSVW intermediate IR utility**: the existing CSVW thin parser (Phase 3
  CORE-05, ~500 LOC) is still useful as INTERNAL IR — a host that fetches a
  CSVW JSON-LD blob could still parse it server-side and pass the result through
  the same downstream surface, just not as a user-facing requirement.

## Decision

We will drop the user-facing CSVW model and replace it with host-side
introspection feeding a new `InferredDescriptor` concrete type.

1. **Introduce `InferredDescriptor` in `fossil-descriptors-input`** as a concrete
   struct (Send + Sync + Clone + Debug + Hash + Eq + serde::{Serialize, Deserialize})
   that implements the existing `InputDescriptor` trait. It is NOT a trait object
   — Salsa interns concrete types only (CLAUDE.md hard rule). Shape:
   ```rust
   struct InferredDescriptor {
       source_name: SmolStr,         // e.g. "users" from `users := io.csv(...)`
       columns: Vec<InferredColumn>, // ordered: column-position fallback
       content_hash: String,         // opaque host-provided hex
   }
   struct InferredColumn {
       name: SmolStr,      // DuckDB column_name
       primitive: SmolStr, // "Integer" | "String" | "Float" | "Bool" | "Date" | ...
                            // (matches fossil-hir::infer::primitive_from_name)
   }
   ```
   The `content_hash` is opaque to Rust; the host (browser DuckDB-WASM or native
   CLI) supplies it. When empty, the consumer derives a deterministic hash from
   the canonical (name, primitive) tuple list — sufficient for Salsa keying.

2. **Deprecate `CsvwDescriptor` as a user-facing entry path, retain as
   intermediate IR.** The existing CSVW thin parser (`crates/fossil-descriptors-input/src/csvw.rs`)
   stays in the codebase and stays callable. The deprecation surfaces in the
   checker: when a source binding declares an explicit `schema = "..."`
   argument, the checker emits diagnostic
   `D-CSVW-DEPRECATED: explicit CSVW descriptor is deprecated; types will be
   inferred from the file directly. Remove the descriptor argument.`
   Severity = warning; the compile still succeeds, using the existing CSVW
   path. Hard removal of the grammar production is deferred to v0.3 / v1.0
   (out-of-scope for v0.2 per CONTEXT.md `## Deferred Ideas`).

3. **NEW WASM API entry point: `FossilPlayground::register_inferred_descriptor`.**
   ```rust
   #[wasm_bindgen]
   impl FossilPlayground {
       pub fn register_inferred_descriptor(
           &mut self,
           descriptor_json: &str,
       ) -> Result<(), JsError>;
   }
   ```
   The browser host runs `DESCRIBE read_csv_auto('<resolved-url>')` via
   DuckDB-WASM, serialises the result to an `InferredDescriptor` JSON blob,
   and calls this method BEFORE `compile()`. The Rust side stashes the descriptor
   in workspace state keyed by `source_name`; `fossil-hir` reads it during
   forward propagation (plan 13-02). API joins the `FossilPlayground` class
   surface defined by ADR-0024.

4. **Native CLI pre-introspection (plan 13-04a).** `fossil compile
   examples/hello.fossil` parses the source, extracts each `io.csv("...")` /
   `io.json("...")` reference, runs `DESCRIBE read_csv_auto(?)` via the
   `duckdb` crate, builds `InferredDescriptor` instances, and registers them
   on the FossilDb's NativeSystem BEFORE typecheck. The walking-skeleton
   invariant (`fossil compile examples/hello.fossil` produces byte-identical
   manifest+parquet vs. Phase 12 baseline) is preserved BOTH WAYS — with the
   sidecar `hello.csvw.json` present (legacy path, D-CSVW-DEPRECATED warning)
   AND without (inferred path).

5. **Salsa invariant preservation.** Descriptors are NOT `salsa::input`. They
   live behind the existing `fossil_base::System` abstraction (ADR-0003,
   ADR-0020), accessed via a new `inferred_descriptor(source_name)` method.
   This mirrors the `read_file` pattern: hosts inject data via System; Rust
   reads it via `&dyn System` from inside Salsa queries. The
   `MAX_PER_MAPPING_FAN_OUT = 1` invariant from Phase 2 SC#2 holds because
   the host registers descriptors AHEAD of `compile()` — synchronous from
   Salsa's perspective. Plan 13-02 verifies via the existing
   `tests/invalidation_regression.rs`.

## Consequences

### Positive

- **Promise restored.** "10 seconds to a triple" is back: the user writes
  `io.csv("path")` and the compiler does the rest.
- **CSVW panel disappears.** `packages/playground/src/csvw/` is deleted
  (plan 13-04b); the playground no longer asks the user to think about
  descriptor metadata.
- **Permalink v0.1 backwards-compat preserved.** The permalink decoder
  silently ignores a `csvw` field when present (plan 13-05); v0.2 encoder
  never emits it.
- **Layer separation maintained for the WASM compiler set.** The 9 gated
  WASM crates remain network-IO-free; the WASM gate stays green.
- **CSVW IR utility retained.** Hosts that need CSVW-driven inference (e.g.
  a future Keasy connector that fetches a CSVW JSON-LD blob from a metadata
  registry) can keep using `CsvwDescriptor` server-side. The intermediate
  IR was always valuable; only the user-facing entry path was lossy.

### Negative

- **API complexity.** Callers MUST register descriptors before `compile()`.
  Mitigated by playground orchestration (plan 13-04b) handling it
  automatically + native CLI doing it transparently (plan 13-04a). The
  raw `compile()` API surface remains the same; the new requirement is
  one extra method call per source.
- **Async surface in the playground.** Browser introspection is necessarily
  async (DuckDB-WASM is Worker-backed, ADR-0026). The playground React
  component (plan 13-04b's `useInferredDescriptors` hook) absorbs this
  complexity behind a `useEffect`; consumers of the React component see
  no change.

### Layer separation scope (IMPORTANT clarification)

The "Rust does no network IO" invariant applies to the **WASM-gated compiler
crate set**: `fossil-base`, `fossil-syntax`, `fossil-hir`, `fossil-mir`,
`fossil-codegen`, `fossil-sinks`, `fossil-ide`, `fossil-ide-db`, `fossil-wasm`.
These crates must build for `wasm32-unknown-unknown` and operate purely on
host-injected data via the `System` trait.

The **native `fossil-cli` is explicitly NOT in this gated set**. It is
native-only by design (ADR-0002 + CLAUDE.md cfg-tripwire), links `duckdb`
natively (CLAUDE.md stack pin `duckdb = "1.10502"` with `features = ["bundled"]`),
and DuckDB's `read_csv_auto` accepts the full DuckDB IO surface — `file://`,
`http(s)://`, and `s3://` URLs (per https://duckdb.org/docs/extensions/httpfs).
Pre-introspection running in fossil-cli MAY therefore touch HTTP / S3 paths
if the user's `.fossil` file references such URLs. This is acceptable because:

(a) the user explicitly asked for it by writing the URL;
(b) fossil-cli is never bundled into the WASM bundle;
(c) the WASM playground path uses DuckDB-WASM (browser-side) which has its
own network policy controlled by the page's origin + CORS rules — not Rust.

The architectural invariant remains "the WASM compiler set does not initiate
network IO"; the native CLI may, transparently, on the user's behalf.

### Breaking

- **None for v0.1 .fossil files.** They still parse + compile (with
  `D-CSVW-DEPRECATED` warning). The grammar production for `schema = "..."`
  remains accepted; the checker just delegates to the CSVW path AND emits
  the warning.
- **None for the WASM public API.** `register_inferred_descriptor` is
  additive (joins the FossilPlayground class surface per ADR-0024); the
  existing `compile_file` / `check` / `diagnostics_for` methods are unchanged.
- **None for v0.1 permalinks.** Decoder accepts the `csvw` field and silently
  ignores it (plan 13-05).

### Cross-references

Partially supersedes ADR-0007 (cap CSVW Metadata Vocabulary support at a
literal-`@context` JSON-LD subset) for the user-facing surface: v0.2 drops
the user-facing CSVW model, but the intermediate IR + thin parser ADR-0007
specifies remain in the codebase as deprecated-but-functional.

References:
- ADR-0002 (15-crate workspace layout — `fossil-cli` is native-only, `fossil-wasm`
  is the WASM bundle entry).
- ADR-0003 (Db trait + System abstraction — `InferredDescriptor` is accessed
  via System, never as a Salsa input).
- ADR-0020 (Db descriptor accessor wiring — `inferred_descriptor` joins the
  same accessor pattern as `output_descriptor`).
- ADR-0024 (fossil-wasm workspace API surface — `register_inferred_descriptor`
  joins the FossilPlayground class).
- ADR-0025 (DuckDB-WASM exact pin — host-side introspection runs against
  exactly this version).
