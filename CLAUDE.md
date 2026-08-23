# Fossil — Claude Code project memory

This file is loaded into every Claude Code session in this repo.
Keep it under 200 lines, rules-not-context.

## Read These First

- `grammar.bnf` — the syntax, normative, and ahead of the parser on purpose. A production
  written here and absent from `crates/fossil-syntax` is work outstanding, not an error in
  the file.
- `apps/docs/` — the language. `/docs/design` is where an argument lives (with
  `design/discarded` for every rejected alternative and what would bring it back, and
  `design/prior-art` for every source named); `/docs/book/typing` is the static semantics,
  the grammar's sibling; `/docs/characteristics` is per-feature and carries both registers.
- `apps/corpus/` — the artifact, with executable guards instead of prose about it.
- `apps/docs/programs/` — the conformance programs. Documentation transcludes them; nothing
  retypes a program into prose. No number here: `crates/fossil-engine/tests/programs.rs` walks
  the directory, and the count in this line was already wrong.
- `crates/` — the crate list. There is no number to quote; `cargo xtask wasm-check` prints
  the wasm32 subset it derived from the dependency graph.
- `apps/docs/CLAUDE.md` — the editorial rules for the docs app, including the two-register
  discipline (where fossil is going / what is true today, never mixed in one sentence).

**There is no second reference.** When a document disagrees with the tree, the tree wins,
and the document is wrong and gets fixed — not annotated, not superseded by a record kept
somewhere else.

## Checks

**CI runs them. Do not run the whole chain locally for every edit** — it is six gates over a
1373-second CPU build, and running it after each comment change is how an afternoon
disappears. Run the narrowest thing that could catch what you just did: one crate, one test.

The full chain and its flags are in `CONTRIBUTING.md`, and the two that are not obvious live
there with their reasons: `--all-targets` (without it `check` skips `tests/`) and
`--no-fail-fast` (without it `test` stops at the first failing suite and reports what it
happened to reach as if it were the workspace).

The WASM gate is the exception worth running by hand before a commit that touches a
compiler-core crate, because it is the one CI failure that is expensive to discover late. It
needs a wasm-capable `clang`; Apple's is not one. `CONTRIBUTING.md` has the invocation.

## Hard Rules

- **WASM gate is non-negotiable.** A PR that breaks `cargo xtask wasm-check` does not merge.
  No exceptions, no `continue-on-error`. Do not write the gated crate set down anywhere: xtask
  derives it from the cdylib dependency closure and prints it, precisely because the two
  hand-maintained `-p …` lists that preceded it had already drifted apart (9 crates vs 6).
- **Forbidden crates** (banned in `deny.toml`): `serde_yml` (RUSTSEC-2025-0068; use `serde_yaml_ng`),
  `tower-lsp` (unmaintained ~3 years; use `lsp-server`, as all three reference implementations do),
  `wasm-pack` (archived; use `wasm-bindgen-cli` + Vite),
  `sqlx` (not WASM-compatible).
- **`tokio` never reaches a wasm build, and that is the whole rule.** A crate may hold it behind
  `cfg(not(target_arch = "wasm32"))`, or as a dev-dependency, or unconditionally if it sits
  outside the wasm closure — and it must say which, and why, in a comment beside the dependency
  in its own `Cargo.toml`. **This rule names no crate, on purpose.** It read "no tokio outside X"
  for months and X was the one crate that had none, because the set was kept by hand.
  `crates/xtask/tests/tokio_placement.rs` derives it instead, prints the real table on any
  failure, and goes red if a crate name reappears in this bullet.
- **No `Box<dyn Trait>` inside Salsa queries.** Salsa interns concrete types; trait objects break
  memoization. Use `&dyn` parameters or enum dispatch.
- **`unsafe_code = "deny"`** at workspace level, not `"forbid"`. Per-item `#[allow(unsafe_code)]` is permitted ONLY at third-party-trait integration boundaries (Salsa Update for rowan types; future FFI), and MUST carry a one-line justification comment naming what the unsafe is for and why no safe alternative exists. Reviewers reject unjustified additions.
- **Where a decision goes:** any decision between alternatives that took >15 minutes gets written
  down within 24h, **in the reference** — the page that states the rule, with the rejected
  alternative and what would bring it back in `design/discarded`, the sources named in
  `design/prior-art`, and any number beside the claim it supports. `CONTRIBUTING.md` has the
  reasoning, which is 159 dead citations and 18 of 20 pointing at the wrong record. **Do not
  reintroduce a directory of records.**
- **pnpm + cargo coexist at repo root.** Rust contributors don't need pnpm; JS/TS contributors need pnpm 9.x + Node 20+. The Rust workspace (`crates/`) and the pnpm workspace (`packages/` + `apps/`) are independent; CI runs them in parallel matrices.
- **`RETURNING.md` ritual:** before stepping away from the project for >1 week, write/update
  `RETURNING.md` (gitignored, local-only) describing current state, what's broken, next 3 steps,
  what NOT to do because tried-it. Read on return before any code change. Solo plus an open
  timeline means breaks are inevitable, and without the ritual the third one leaves the codebase
  opaque to its own author.
- **Walking-skeleton invariant:** `fossil run examples/hello.fossil --dest <tmp>` must keep
  producing a valid GraphAr dataset — 5 `Person` vertices, asserted by content, not existence.
  `crates/fossil-cli/tests/walking_skeleton.rs` is the test that goes red. A refactor that
  breaks it for >3 days is reverted and broken into smaller steps.

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
| wasm-opt (binaryen) | 116 via `cargo install wasm-opt@0.116.1` | NOT apt (ubuntu ships binaryen 108, whose wasm-opt corrupts wasm-bindgen's externref table → `Table.grow(): failed to grow table` instantiating the graph wasm on Node 20, binaryen #4711; 116 fixes it). `packages/executor/scripts/build-wasm.sh` passes the six wasm32 default features (bulk-memory, sign-ext, mutable-globals, nontrapping-fptoint, reference-types, multivalue — Rust 1.87/LLVM 20). NOT `-all` → no gc/typed-funcref, which break instantiation |
| serde_yaml_ng | 0.10 | NOT serde_yml (RUSTSEC) |
| lsp-server | 0.7 | NOT tower-lsp (unmaintained) |
| arrow + parquet | 58 | for GraphAr writer (no Apache GraphAr Rust SDK exists) |
| shex_ast + rudof_iri | 0.3 | ShEx target shapes; explicit features only — `default-features` drags in what wasm32 cannot build |
| datafusion | 54, `default-features = false` | pinned in `crates/fossil-df/Cargo.toml`, not the workspace table — see the `zstd`/`arrow-ipc` note there |

When bumping: update workspace `Cargo.toml` `[workspace.dependencies]`, run `cargo deny check`,
verify WASM gate, and write the reason down on the page it affects if it is a major version with
API changes.

## Project Layout

`ls crates/` and `git ls-files packages` are the lists. This is what each one is for; a
`[NATIVE-ONLY]` crate carries a `wasm32` `compile_error!` tripwire.

```
crates/
  fossil-base/             Salsa Db trait + System abstraction (thin Db, fat System)
  fossil-syntax/           lossless CST + parser
  fossil-hir/              types + name resolution + bidirectional checker + the stdlib
                           catalog (`stdlib.rs` — the checker resolves calls against it)
  fossil-mir/              typed operator algebra; `src/op.rs` is the operator enum
  fossil-shex/             ShExDescriptor over `shex_ast` — backward target-shape checking
  fossil-descriptors-{input,output}/   trait + impls (ShEx, inferred)
  fossil-resolver/         host-injected cloud path resolution (s3://, az://)  [NATIVE-ONLY]
  fossil-lineage/          source lineage + provider introspection, projected onto the wire
  fossil-sinks/            the canonical GraphAr manifest model (atop arrow + parquet)
  fossil-df/               DataFusion backend for the property-graph MIR
  fossil-engine/           fossil's native HOST: the `System` the compiler runs against, the
                           shape documents a program names, and the compile→run pipeline.
                           `fossil-wasm` is the same shape for the browser. It no longer
                           introspects or installs `@conn` secrets  [NATIVE-ONLY]
  fossil-introspect/       those two, which are a host job and not a compiler one: `DESCRIBE`
                           each source's columns, and the `--creds-stdin` payload that
                           authenticates one. `fossil-cli` calls it before the compile. The one
                           crate linking `DuckDB` on a normal edge — and it is native by that
                           edge and by `fossil-resolver`, without a tripwire of its own
  fossil-layout/           the layout post-pass — Louvain + Morton over Parquet through
                           arrow-rs, linking no engine (`DuckDB` is a dev-dependency). It
                           COMPILES for wasm32 and declares it with `[package.metadata.fossil]
                           wasm = true`, which is what puts it in the gate closure. It was
                           `fossil-runtime`, and it is not a runtime: the crate IS the pass
  fossil-mem-probe/        `FOSSIL_MEM_PROBE` — peak RSS + elapsed seconds per phase of a
                           write. Depends on NOTHING; both halves of the write path
                           (fossil-df, fossil-layout) report through it
  fossil-run-status/       the `fossil run --output-json` wire contract
  fossil-graph-schema/     the canonical graph-schema — the shared substrate contract
  fossil-graph/            the typed verb surface over GraphAr+DuckDB (WASM-clean)
  fossil-mcp/              that same verb surface as a native server-side service
  fossil-ide/              hover, completion, goto-def + the symbol/prefix/workspace indexes
  fossil-cli/              `fossil check/run/providers/refs`  [NATIVE-ONLY]
  fossil-lsp/              LSP server via lsp-server  [NATIVE-ONLY]
  fossil-wasm/             WASM host shim (FossilPlayground API + the tokenizer the editor reuses)
  fossil-df-wasm/          the fossil-df executor exposed to JS
  fossil-graph-wasm/       wasm-bindgen binding for the fossil-graph verb surface
  xtask/                   repo automation; `cargo xtask wasm-check` is its one command

packages/                  npm-published @fossil-lang/* family (pnpm workspace)
  wasm/                    wraps fossil-wasm build outputs (.js + .wasm + .d.ts)
  graph/                   two halves over one manifest. The verbs are the root barrel and NEED
                           the gitignored `pkg/`; the addressing (`resolveCorpus` — no WASM) is
                           `@fossil-lang/graph/address`, and it has a subpath because a barrel
                           import is not one. `tests/address-standalone.test.ts` proves it.
  executor/                datafusion-wasm query executor
  types/                   shared TS types (SourceRef, ConnectionResolver, FossilTheme — zero runtime)
  resolvers/               default + mock + public-HTTP ConnectionResolver impls
  introspect/              source-binding schema introspection (the one home; `fossil-introspect`
                           is the Rust sibling). Their agreement is ENFORCED, not asserted:
                           `packages/introspect/tests/rust-parity.test.ts` derives the regex, the
                           reader arms and the type table out of
                           `crates/fossil-introspect/src/lib.rs` and fails on drift. It is a
                           **pnpm** test — editing that Rust turns it red and `cargo test` will
                           not tell you.

apps/                      NOT published, and no RECURSIVE CI step reaches them (all are
                           filtered to `./packages/*` by path — see release.yml). Each gets its
                           own path-filtered workflow instead: `docs.yml`, `corpus.yml`.
  docs/                    Next.js + fumadocs. Where fossil is GOING, with what is already
                           true marked as such; `apps/docs/CLAUDE.md` has the editorial rules
  corpus/                  the artifact: the format's conventions with executable guards, the
                           conformance corpus, and a reader that shares no code with the guards

grammar.bnf                the syntax, normative, and ahead of the parser on purpose
tests/wasm_parity/         the manual DuckDB-WASM cross-engine parity harness
```

The `ui/ viewer/ editor/ codemirror-fossil/` React family moved to `@kanzo-tech/*`; commit
`873cbc0` deleted all four. They are gone — fossil ships no UI.

## Style

- Prefer enum dispatch over `Box<dyn Trait>`. Salsa interning needs concrete types.
- `Result<T, E>` with thiserror-style enums in lib crates; `miette::Result` in CLI / LSP / runtime.
- `tracing` for structured logs (not `log`). `RUST_LOG=fossil=debug` is the canonical filter.
- Snapshot tests via `insta` — `git ls-files '*.snap'` shows which crates carry them (not the
  parser CST: too brittle).
- Doc comments on `pub` items in compiler-core crates.

## Common Tasks

- **Add a new dependency:** add to `[workspace.dependencies]` (workspace-level), then per-crate
  `dep = { workspace = true }`. Run `cargo deny check` to confirm no advisory or license issue.
  Run WASM gate to confirm no transitive WASM-incompat leak. If the dep is foundational, say why
  on the page whose claim depends on it.
- **Add a new crate:** `members = ["crates/*"]` picks it up; copy the shape of an existing
  peer (compiler-core, native-only with the `wasm32` cfg-tripwire, or wasm-shim). Do not
  record the new crate count anywhere — nothing should have one to update.
- **Run the WASM smoke test:** build the nodejs bindgen target, then
  `node crates/fossil-wasm/test-wasm-workspace.js` — the build precondition is in that file's
  header. `crates/fossil-wasm/tests/workspace.rs` is its native mirror and runs on every PR.
- **Add an app:** it goes under `apps/`, carries `private: true`, and needs nothing else — every recursive CI step and every root script is already filtered to `./packages/*`, so an app cannot be version-stamped or published by accident. Give it its own path-filtered workflow rather than a step in `pnpm-ci.yml`, whose 60-minute ceiling exists for a cold cargo build.

## Anti-patterns

- Using `default-features = true` on rudof crates — be explicit about what you opt into; the
  defaults drag in crates that do not build for wasm32.
- Putting compiler logic in `fossil-base` — it is the trait + db substrate, no business logic.
- Skipping the WASM gate locally. CI catches it eventually but the feedback loop is slower.
- Editing a generated file (`packages/wasm/pkg/*`, `crates/fossil-wasm/pkg/*`, or `target/*`). They are regenerated.
