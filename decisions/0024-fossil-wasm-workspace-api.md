# ADR 0024: `fossil-wasm` is the LSP-Worker; ship a `ty_wasm`-shaped Workspace API

**Date:** 2026-05-23
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/phases/07-wasm-api-duckdb-wasm-base-playground/07-RESEARCH.md` §"Workspace API" + §"LSP-over-postMessage"
- `decisions/0001-use-lsp-server-not-tower-lsp.md` (the `lsp-server` choice that makes `fossil-lsp` native-only)
- `decisions/0020-db-descriptor-accessor-wiring.md` (the `HirDb::output_descriptor_kind` accessor `set_target_shex` swaps)
- `decisions/0022-salsa-cancellation-model.md` (the `Setter` revision-bump mechanism `update_file` reuses)
- `decisions/rudof-wasm.md` (Phase 0 spike outcome — `shex_ast` + `rudof_iri` + `prefixmap` are WASM-clean)
- Astral `ty_wasm` upstream: `crates/ty_wasm/src/lib.rs` `impl Workspace { open_file / update_file / close_file }`

## Context

The ROADMAP wording for Phase 7 says "`fossil-lsp` running in a Web Worker
as WASM". Taken literally, that's not the design — `fossil-lsp` carries a
`compile_error!` cfg-tripwire (`#[cfg(target_arch = "wasm32")]
compile_error!("fossil-lsp is native-only ...")`) because it uses
`lsp-server` (crossbeam-channel + stdio) per ADR-0001 and `tokio` would
be banned anyway by CLAUDE.md's hard rules. Neither dependency compiles
to `wasm32-unknown-unknown`.

The playground STILL needs an LSP server-side in the browser so Monaco
can run language features (hover / completion / goto-def / diagnostics).
The seam that does work is:

- `fossil-ide` is a WASM-clean crate (the 7th in the 9-crate gate; Phase
  6 LSP-01) that exposes free functions (`hover`, `goto_definition`,
  `completions`, `code_actions`, `document_symbols`, `semantic_tokens`,
  `line_index`, ...) — no transport, no IO.
- `fossil-lsp` is the native LSP transport: a thin dispatch loop that
  wires each LSP method to a `fossil-ide` free function (`stdio →
  fossil-ide → stdio`).
- The browser needs an EQUIVALENT thin dispatch layer that wraps the
  same `fossil-ide` free functions but speaks LSP-JSON-RPC over
  postMessage (`postMessage → fossil-ide → postMessage`).

Astral's `ty_wasm` (the WASM build of `ty`, ruff's type checker) has
solved this exact problem with a small `Workspace` API the JS side
drives:

```rust
impl Workspace {
    pub fn open_file(&mut self, path: &str, contents: &str) -> Result<FileHandle, Error>;
    pub fn update_file(&mut self, &FileHandle, contents: &str) -> Result<(), Error>;
    pub fn close_file(&mut self, FileHandle) -> Result<(), Error>;
    pub fn check(&self) -> Vec<Diagnostic>;
    // ...
}
```

A `FileHandle` is a small `u32` newtype on the JS boundary; a
`HashMap<FileHandle, SourceFile>` translates handles → Salsa-interned
inputs internally. `update_file` is the critical method — it mutates
the SAME interned `SourceFile` via the Salsa `Setter` (`set_text`),
which BUMPS THE REVISION (ADR-0022 — the real cancellation trigger).
This is EXACTLY the mechanism Phase 6's LSP `didChange` path uses
(`fossil-lsp::LspState::change`, plan 06-09). No new tracked queries,
fan-out=1 stays unchanged.

Three options were on the table for the WASM-side LSP server:

| # | Option | Verdict |
|---|--------|---------|
| 1 | Recompile `fossil-lsp` to WASM | REJECTED — `lsp-server` not WASM-portable; cfg-tripwire is there for a reason; ADR-0001 |
| 2 | Hand-roll the LSP transport in pure TS without a Rust dispatch loop | REJECTED — forces every feature function to be reimplemented in TS; defeats the "one crate, two hosts" model (PROJECT.md EXT-01) |
| 3 | Grow `fossil-wasm` to wrap `fossil-ide` + speak LSP over postMessage (the `ty_wasm` model) | ACCEPTED — this ADR |

The third option also gives the playground a clean Rust API for the
non-LSP playground panels (the run-button calls `compile_file(handle)`
directly without an LSP `executeCommand` round-trip; the Schema panel
calls `set_target_shex(text)` directly to install a ShEx schema).

## Decision

`fossil-wasm` IS the LSP server-side in the browser. It grows the
`ty_wasm`-shaped `Workspace` lifecycle API on `FossilPlayground`:

```rust
#[wasm_bindgen]
impl FossilPlayground {
    pub fn open_file(&mut self, path: String, contents: String) -> Result<FileHandle, JsError>;
    pub fn update_file(&mut self, handle: FileHandle, contents: String) -> Result<(), JsError>;
    pub fn close_file(&mut self, handle: FileHandle) -> Result<(), JsError>;

    pub fn check(&self) -> Result<JsValue, JsError>;                    // workspace-wide drain
    pub fn diagnostics_for(&self, handle: FileHandle) -> Result<JsValue, JsError>;  // B3 per-file drain
    pub fn compile_file(&self, handle: FileHandle) -> Result<JsValue, JsError>;

    pub fn set_target_shex(&mut self, text: &str) -> Result<(), JsError>;
}
```

`FileHandle` is a `u32` newtype — NEVER a Salsa key, NEVER inside
`Box<dyn>`. The internal `OpenFiles` is `HashMap<FileHandle, SourceFile>`
plus a `by_uri` secondary index for the LSP Worker's URI → handle lookup.

`set_target_shex` parses the ShEx text via
`fossil_descriptors_output::ShExDescriptor::from_reader` and swaps a
fresh `Arc<OutputDescriptorKind>` into the host `WasmDb`'s descriptor
slot — atomically, with retain-on-failure (mirroring
`fossil-lsp::load_sibling_shex` in 06-09). The descriptor accessor is
on `HirDb` per ADR-0020 (NOT on `Db::system()` per ADR-0006), so we
introduce a fresh `WasmDb` struct that owns the `Arc<OutputDescriptorKind>`
storage and impls `HirDb` for it. `WasmDb` mirrors `LspDb`
byte-for-byte.

The Phase-1 `compile(source: &str)` + `classification()` methods are
RETAINED — Phase-1 test-wasm.js + the STDL-07 classification path keep
working without change.

`fossil-lsp` is NOT recompiled. Its `compile_error!` cfg-tripwire stays
non-negotiable.

## Consequences

**Positive:**

- ONE crate (`fossil-ide`), TWO hosts (`fossil-lsp` native stdio +
  `fossil-wasm` WASM postMessage). Validates the "one crate, two hosts"
  vision (PROJECT.md EXT-01) — also reused by the VS Code extension in
  Phase 9 without further work.
- The `update_file` path reuses the EXACT mechanism Phase 6's LSP
  `didChange` uses (Salsa `Setter` — ADR-0022). No new tracked queries
  land, `MAX_PER_MAPPING_FAN_OUT` stays at 1 (verified by
  `invalidation_regression`).
- `fossil-wasm` gains `fossil-ide` (LineIndex for UTF-16 byte → LSP
  range conversion) and `salsa` (WasmDb needs `Storage<Self>`) as
  direct deps. Both wasm32-clean; the 9-crate WASM gate stays at 9
  crates (no new gated crate).
- The architectural seam (recompile? hand-roll TS? `ty_wasm` pattern?)
  is captured in ONE ADR; future evolution (the VS Code extension
  activation in Phase 9, second-language playground integrations,
  alternative editors) reuses the seam without further design work.
- Native-side cargo-test mirror (`crates/fossil-wasm/tests/workspace.rs`)
  catches API regressions on every PR through pure-Rust `*_native` /
  `*_rows` / `*_result` helpers exposed alongside the `#[wasm_bindgen]`
  surface — no JS toolchain needed on the per-PR path.

**Negative:**

- ROADMAP Phase 7 wording is RECONCILED by this ADR (not deleted —
  rephrased as "the playground exposes LSP features via `fossil-wasm`
  speaking LSP over postMessage"); the 07-SUMMARY at phase close
  re-states the seam in the per-phase summary.
- `fossil-wasm` ships TWO surfaces per public method: the `#[wasm_bindgen]`
  wrapper (JS-facing, JsValue + JsError) and the pure-Rust `*_native` /
  `*_rows` / `*_result` helper (cargo-test-facing). The split mirrors
  the Phase-5 `classification()` ↔ `stdlib_classification()` precedent —
  established pattern, but doubles the surface area cosmetically.
- A bug in the postMessage dispatch loop (07-03) is hidden from `cargo
  test` — the JS-side rehearsal (`test-wasm-workspace.js`) is the only
  way to catch it. Mitigated by the CI playground job in 07-09 which
  drives the node script on every PR.

**Neutral:**

- The LSP-over-postMessage transport (the dispatch loop itself) is
  07-03's scope, not this plan's. 07-02 publishes the API surface;
  07-03 wires the JSON-RPC ↔ Workspace-method bridge.
- The `FileHandle` is intentionally NOT the Salsa interned id (which
  doesn't cross the JS boundary cleanly because of the `'db` lifetime).
  The translation cost (a HashMap lookup per call) is negligible vs.
  the per-call Salsa + checker work.

## Alternatives considered

1. **Recompile `fossil-lsp` to WASM.** REJECTED. `lsp-server` uses
   `crossbeam-channel` (not WASM-portable) and stdio (not available in
   browsers). The `compile_error!` cfg-tripwire encodes this — removing
   it would force `fossil-lsp` to grow a Web-Worker transport in
   addition to its stdio transport, which is exactly the duplication
   "one crate, two hosts" was designed to avoid.

2. **Hand-roll an LSP transport in pure TS without a Rust dispatch
   loop.** REJECTED. The dispatch loop is where all the feature
   functions live (`hover` → `fossil_ide::hover_bidirectional`, etc.).
   Reimplementing those in TS would re-derive the `Salsa::Setter`
   semantics + the descriptor wiring + the LineIndex UTF-16 conversion
   on the JS side — duplicate engineering forever, with drift risk.

3. **Use `tower-lsp` for the WASM-side LSP transport.** REJECTED per
   ADR-0001 — `tower-lsp` is unmaintained.
