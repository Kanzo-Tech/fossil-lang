# Fossil — Claude Code project memory

This file is loaded into every Claude Code session in this repo.
Keep it under 200 lines, rules-not-context.

## Read These First

- `decisions/` — ADRs (Nygard format). Every non-obvious choice is recorded here.
  Most relevant: ADR-0001 (LSP framework), ADR-0002 (15-crate layout), ADR-0003 (Db trait shape).
  Also `decisions/rudof-wasm.md` (Phase 0 spike outcome).
- `.planning/PROJECT.md` — current scope, constraints, key decisions, out-of-scope list.
- `.planning/ROADMAP.md` — 10 phases of Milestone 1 (compiler + LSP + playground + extension).
- `.planning/STATE.md` — current focus, in-flight work.
- The 5 design docs in repo root (`architecture.md`, `grammar.bnf`, `type-system.md`,
  `operator-algebra.md`, `stdlib.md`) — original design corpus. Some points superseded
  by research synthesis (see `.planning/research/SUMMARY.md`); when in doubt, ADRs win.

## Build & Test Commands

```bash
cargo check --workspace                                          # native, all crates
cargo test --workspace                                           # native tests
cargo fmt --all -- --check                                       # format check
cargo clippy --workspace --all-targets -- -D warnings            # lint check
cargo deny check                                                 # advisories + licenses + bans
cargo check --target wasm32-unknown-unknown \
    -p fossil-base -p fossil-syntax -p fossil-hir \
    -p fossil-mir -p fossil-codegen -p fossil-sinks \
    -p fossil-ide -p fossil-ide-db -p fossil-wasm  # WASM gate (9 crates)
```

CI runs all of the above on every PR. Locally, the WASM gate is the highest-leverage
check — run it before any commit that touches a compiler-core crate.

## Hard Rules

- **WASM gate is non-negotiable.** A PR that breaks `cargo check --target wasm32-unknown-unknown`
  on any of the 9 gated compiler crates does not merge. No exceptions, no `continue-on-error`.
- **Forbidden crates** (banned in `deny.toml`): `serde_yml` (RUSTSEC-2025-0068; use `serde_yaml_ng`),
  `tower-lsp` (unmaintained; use `lsp-server` per ADR-0001), `wasm-pack` (archived; use `wasm-bindgen-cli` + Vite),
  `sqlx` (not WASM-compatible).
- **No compiler logic in Phase 0.** Phase 0 is workspace genesis only — every crate is a stub.
  Real implementation begins Phase 1.
- **No `tokio` outside `fossil-lsp`.** And `fossil-lsp` is native-only with a `compile_error!` cfg-tripwire.
- **No `Box<dyn Trait>` inside Salsa queries.** Salsa interns concrete types; trait objects break
  memoization. Use `&dyn` parameters or enum dispatch.
- **`unsafe_code = "deny"`** at workspace level (per ADR-0004). Per-item `#[allow(unsafe_code)]` is permitted ONLY at third-party-trait integration boundaries (Salsa Update for rowan types; future FFI), and MUST carry a one-line justification comment naming what the unsafe is for and why no safe alternative exists. Reviewers reject unjustified additions.
- **ADR ritual:** any decision between alternatives that took >15 minutes gets an ADR within 24h.
  Use `decisions/template.md`, file naming `NNNN-verb-noun-phrase.md`. See ADR-0001/0002/0003 as examples.
- **`RETURNING.md` ritual:** before stepping away from the project for >1 week, write/update
  `RETURNING.md` (gitignored, local-only) describing current state, what's broken, next 3 steps,
  what NOT to do because tried-it. Read on return before any code change. Mitigates P-SOLO-2.
- **Walking-skeleton invariant** (post-Phase 1): at no point should `fossil compile examples/hello.fossil`
  regress. A refactor that breaks the e2e demo for >3 days is reverted and broken into smaller steps.

## Stack Pins

| Component | Version | Notes |
|-----------|---------|-------|
| Rust toolchain | 1.90 | per `rust-toolchain.toml`; bumped from 1.85 to unblock wasm-bindgen-cli 0.2.120 install |
| salsa | 0.26 + `accumulator` feature | incremental query framework |
| rowan | 0.16 | lossless CST |
| logos | 0.16 | lexer |
| miette | 7.6 | diagnostics |
| sqlparser | 0.59 | SQL AST construction |
| duckdb | 1.10502 (`features = ["bundled"]`) | native execution |
| wasm-bindgen | =0.2.120 | exact pin; CLI must match |
| serde_yaml_ng | 0.10 | NOT serde_yml (RUSTSEC) |
| lsp-server | 0.7 | per ADR-0001 (NOT tower-lsp) |
| arrow + parquet | latest | for GraphAr writer (no Apache GraphAr Rust SDK exists) |

When bumping: update workspace `Cargo.toml` `[workspace.dependencies]`, run `cargo deny check`,
verify WASM gate, file ADR if it's a major version with API changes.

## Project Layout

```
crates/
  fossil-base/             Salsa Db trait + System abstraction (per ADR-0003)
  fossil-syntax/           lossless CST + parser
  fossil-hir/              types + name resolution + bidirectional checker (collapsed per ADR-0002)
  fossil-mir/              typed operator algebra (11 ops)
  fossil-codegen/          MIR → DuckDB SQL + GraphAr manifest
  fossil-descriptors-{input,output}/   trait + impls (CSVW, ShEx)
  fossil-sinks/            Sink trait + GraphAr writer (atop arrow + parquet)
  fossil-registry/         function registry
  fossil-runtime/          DuckDB native execution    [NATIVE-ONLY]
  fossil-ide-db/           symbol indexes
  fossil-ide/              hover, completion, goto-def
  fossil-cli/              `fossil compile/check/run` [NATIVE-ONLY]
  fossil-lsp/              LSP server via lsp-server  [NATIVE-ONLY]
  fossil-wasm/             WASM host shim (FossilPlayground API)

decisions/                 ADRs + non-numbered decision logs (rudof-wasm spike, etc.)
playground-poc/            Phase 0 throwaway WASM smoke test (replaced by playground/ in Phase 7)
.planning/                 GSD orchestration artifacts (gitignored — commit_docs=false)
```

## Style

- Prefer enum dispatch over `Box<dyn Trait>`. Salsa interning needs concrete types.
- `Result<T, E>` with thiserror-style enums in lib crates; `miette::Result` in CLI / LSP / runtime.
- `tracing` for structured logs (not `log`). `RUST_LOG=fossil=debug` is the canonical filter.
- Snapshot tests via `insta` for type-check output and SQL codegen output (not for parser CST yet — too brittle).
- Doc comments on `pub` items in compiler-core crates. Doc comments are not required on Phase 0 stubs.

## Common Tasks

- **Add a new dependency:** add to `[workspace.dependencies]` (workspace-level), then per-crate
  `dep = { workspace = true }`. Run `cargo deny check` to confirm no advisory or license issue.
  Run WASM gate to confirm no transitive WASM-incompat leak. Update ADR if the dep is foundational.
- **Add a new crate:** add to `members = ["crates/*"]` (auto-included), create `Cargo.toml`
  using one of the three stub templates (compiler-core, native-only with cfg-tripwire, or wasm-shim).
  Update ADR-0002 if the crate count changes from 15.
- **Add a new ADR:** copy `decisions/template.md`, increment NNNN. Update `decisions/README.md` index.
- **Run the WASM smoke test:** see `playground-poc/README.md`. Phase 7 replaces this with a Vite project.

## Anti-patterns

- Importing `tokio` anywhere outside `fossil-lsp` (and even there, only `tokio = { version, features = ["sync"] }`).
- Using `default-features = true` on rudof crates — be explicit about what you opt into per the
  rudof spike outcome (`decisions/rudof-wasm.md`).
- Putting compiler logic in `fossil-base` — it is the trait + db substrate, no business logic.
- Skipping the WASM gate locally. CI catches it eventually but the feedback loop is slower.
- Editing a generated file (`playground-poc/pkg/*` or `target/*`). They are regenerated.
