# Fossil — Claude Code project memory

This file is loaded into every Claude Code session in this repo.
Keep it under 200 lines, rules-not-context.

## Read These First

- `grammar.bnf` — the syntax, normative, and ahead of the parser on purpose. A production
  written here and absent from `crates/fossil-syntax` is work outstanding, not an error in
  the file.
- `catalogue.bnf` — WHICH NAMES EXIST, both halves: the `io.` constructors and every stdlib
  function with its signature and lowering, plus the DuckDB type table both profiling
  hosts read. Generated from, not compared against — `cargo xtask catalogue` writes every
  projection and no Rust or TS states a row a second time. Adding a function is a line here.
  `CONTRIBUTING.md` has the list and the round-trip guard.
- `docs/` — the whole of the documentation, and there is no second site.
  `/docs/book/getting-started` teaches the language and `/docs/format` specifies the graph;
  behind a maintainers' divider,
  `/docs/design` is where an argument lives, with `design/discarded` for every rejected
  alternative and what would bring it back, and `design/prior-art` for every source named.
- `docs/programs/` — the conformance programs. Documentation transcludes them; nothing
  retypes a program into prose. No number here: `crates/fossil-df/tests/programs.rs` walks
  the directory, and the count in this line was already wrong.
- `crates/` — the crate list. There is no number to quote; `cargo xtask wasm-check` prints
  the wasm32 subset it derived from the dependency graph.
- `docs/CLAUDE.md` — the editorial rules for the docs app: which pages declare a
  `direction:` and which simply describe what is there, and why evidence is transclusion
  rather than a cited line number.

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
- **Forbidden crates** (banned in `deny.toml`): `serde_yml` (RUSTSEC-2025-0068; `serde_yaml_ng` if YAML ever returns),
  `wasm-pack` (archived; use `wasm-bindgen-cli` + Vite),
  `sqlx` (not WASM-compatible).
- **`tokio` never reaches a wasm build from our own manifests.** A crate may hold it behind
  `cfg(not(target_arch = "wasm32"))`, as a dev-dependency, or unconditionally outside the wasm
  closure. `cargo xtask wasm-check` reads the graph cargo resolves for wasm32 and fails on any
  crate in the closure that still depends on it.
- **No `Box<dyn Trait>` inside Salsa queries.** Salsa interns concrete types; trait objects break
  memoization. Use `&dyn` parameters or enum dispatch.
- **`unsafe_code = "deny"`** at workspace level, not `"forbid"`. Per-item `#[allow(unsafe_code)]` is permitted ONLY at third-party-trait integration boundaries (future FFI — the four Salsa `Update` impls for rowan types went with salsa 0.28, whose `SalsaValue` needs none for a `'static` type, and the workspace holds no `unsafe` today), and MUST carry a one-line justification comment naming what the unsafe is for and why no safe alternative exists. Reviewers reject unjustified additions.
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
- **Walking-skeleton invariant:** `examples/hello.fossil`, run through the executor
  (`fossil_df::Executor`, the one write host), must keep producing a readable Apache GraphAr
  graph — 5 `Person` vertices, asserted by content read back with DuckDB through the
  information files, not existence.
  `crates/fossil-df/tests/walking_skeleton.rs` is the test that goes red. A refactor that
  breaks it for >3 days is reverted and broken into smaller steps.

## Stack Pins

| Component | Version | Notes |
|-----------|---------|-------|
| Rust toolchain | 1.90 | per `rust-toolchain.toml`; bumped from 1.85 to unblock wasm-bindgen-cli 0.2.120 install |
| salsa | 0.28.5, defaults minus `accumulator` | incremental query framework; diagnostics travel in query outputs (`/docs/design/incremental`); ≥0.28.5 for RUSTSEC-2026-0308. 0.28 returns by reference by default, so every tracked item that predates it names `returns(clone)` or `returns(copy)`, the 0.26 default |
| rowan | 0.16 | lossless CST |
| logos | 0.16 | lexer |
| miette | 7.6 | diagnostics |
| duckdb | 1.10502 (`features = ["bundled"]`) | native execution |
| wasm-bindgen | =0.2.120 | exact pin; CLI must match |
| wasm-opt (binaryen) | 116 via `cargo install wasm-opt@0.116.1` | NOT apt (ubuntu ships binaryen 108, whose wasm-opt corrupts wasm-bindgen's externref table → `Table.grow(): failed to grow table` instantiating a wasm-bindgen module on Node 20, binaryen #4711; 116 fixes it). `packages/executor/scripts/build-wasm.sh` passes the six wasm32 default features (bulk-memory, sign-ext, mutable-globals, nontrapping-fptoint, reference-types, multivalue — Rust 1.87/LLVM 20). NOT `-all` → no gc/typed-funcref, which break instantiation |
| arrow + parquet | 58 | the graph writer, `fossil_df::write` (through `datafusion`'s re-export) and `graphar-parquet` |
| graphar-info, graphar-parquet | 0.1.0-alpha.1, crates.io | the Apache GraphAr YAML model and chunk writers (Kanzo-Tech/graphar); no `tokio`, builds for wasm32; `serde_yaml_ng`, never `serde_yml` |
| shex_ast, shacl, rudof_rdf, rudof_iri, prefixmap, rml | Kanzo-Tech/rudof by `git` + `rev` (master after its per-syntax features, R-d; the owner tags it later) | ShEx and SHACL shape documents, the Turtle parser, and `rml`, the RML-Core mapping `describe` builds and writes as `mapping.ttl` (its `sql` feature renders it in tests); one git source for all six, `default-features = false`, and `rudof_rdf` with `features = ["turtle"]`, the one syntax fossil reads — the defaults drag in `reqwest`/`tokio` and what wasm32 cannot build |
| datafusion | 54, `default-features = false` | pinned in `crates/fossil-df/Cargo.toml`, not the workspace table — see the `zstd`/`arrow-ipc` note there |

When bumping: update workspace `Cargo.toml` `[workspace.dependencies]`, run `cargo deny check`,
verify WASM gate, and write the reason down on the page it affects if it is a major version with
API changes.

## Project Layout

`ls crates/` and `git ls-files packages` are the lists. This is what each one is for.

```
crates/
  fossil-base/             the Salsa substrate: the `Db` trait and `FossilDb`, every input a
                           host writes (files, sources, connections, the provider rows), the
                           `Diagnostics` sink, and `location` — the ONE rule turning a written
                           reference into something a reader can open: `@conn`, scheme,
                           absolute, else the program's directory; never the cwd
  fossil-syntax/           lossless CST + parser
  fossil-hir/              types + name resolution + bidirectional checker + the stdlib
                           catalog, and `lineage`: what a program reads, through the
                           connections. `stdlib.rs` owns the TYPES a row is written in; the ROWS
                           are `catalogue.bnf` and `stdlib/generated.rs` is what
                           `cargo xtask catalogue` makes of them. Adding a function is a
                           line in the `.bnf` and touches no Rust. And the shape-document
                           decoders: `shex` over `shex_ast`, `shacl` over rudof's `ShaclParser`, and
                           `providers::PROVIDERS`, the registry a compiling host installs
  fossil-mir/              typed operator algebra; `src/op.rs` is the operator enum
  fossil-storage/          a vended `StorageCredential` (Iceberg REST's) → a scoped DuckDB secret,
                           an Azure SAS lend, or (feature `object-store`) an `object_store` store
                           routed by longest prefix and renewed at expires−5min — the one IO path
                           of DataFusion and of every byte fossil reads or writes. `js`: a JS
                           `Host` as its `Host`. The one renderer of a `CREATE SECRET`
  fossil-storage-wasm/     that, exposed to JS for `@fossil-lang/storage`
  fossil-df/               DataFusion backend for the property-graph MIR, the one graph
                           writer (`write`: `layout` lays the tables out as Apache GraphAr's
                           and holds them to what an RDF reader can recover, `describe` builds
                           the RML-Core mapping of them, `graphar-parquet` writes the chunks,
                           `mapping.ttl` after them and `fossil.graph.yml` last), and `Executor` — the
                           whole run a host drives, under the 2 GiB pool in `memory.rs` that
                           refuses as `run/over-budget`. `facts` counts what the footers hold
                           (`fossil:stats`)
  fossil-profile/          what a source is — columns and types, counts, candidate keys, the
                           inclusions between sources, a sample — planned as queries and read
                           from their answers, running NOTHING (sans-IO, `run(sources, options,
                           exec)`). Each query is an `Ask`: the browser runs its SQL on DuckDB-WASM,
                           `fossil-df`'s tests on native `duckdb`, and the writer renders it on
                           DataFusion to count what it wrote; it links no engine, so it is in the
                           wasm closure. There is no native language server: it was deleted on
                           2026-10-12 (`/docs/design/discarded`). `Options::TYPES` is one
                           `DESCRIBE` a source, what the editor asks for. `generated.rs` is
                           `catalogue.bnf`'s reader and type table
  fossil-graph-schema/     the canonical graph-schema — the shared substrate contract — and the
                           error catalogue: `Problem`, `Failure`, and (feature `js`) the one
                           function every wasm crate throws a failure through
  fossil-ide/              hover, completion, goto-def, outline — every one asking `def_map`,
                           the one name resolver, and the checker; no index of its own
  fossil-wasm/             WASM host shim (the workspace under `openProgram` + the tokenizer)
  fossil-df-wasm/          `fossil_df::Executor` exposed to JS — the only host that writes a
                           graph, in the browser and in Node. There is no native CLI: it
                           was deleted on 2026-09-30 (`/docs/design/discarded` says what
                           brings it back)
  xtask/                   repo automation. Two commands: `wasm-check` derives the wasm32
                           subset from the cdylib closure, and `catalogue [--check]`
                           regenerates every projection of `catalogue.bnf`.
                           What is generated from a Rust TYPE — the error catalogue and every
                           wire type in `@fossil-lang/types` — is
                           the expect test `tests/wire.rs` (`UPDATE_EXPECT=1`), because the
                           binary must not link the crates `catalogue` generates into

packages/                  npm-published @fossil-lang/* family (pnpm workspace)
  wasm/                    wraps fossil-wasm build outputs (.js + .wasm + .d.ts); `runProfiler`
                           is the loop over its `Profiler`
  executor/                the datafusion-wasm executor — the one writer of a graph. A run
                           answers `{ location, dropped }`; what it wrote is `<location>fossil.graph.yml`.
                           In a browser it runs in a worker the executor owns (`src/protocol.ts`).
                           `integration/` runs a program through it and reads the graph back with
                           `@kanzo-tech/graphar-duckdb` on DuckDB-WASM (`pnpm test:integration`,
                           `pnpm-ci.yml`); `test` stays `tests/` alone because the release gate runs it
  types/                   Host — `connections()` + `credentials(scope, access)`, each handed
                           `{ signal }` — the one host contract, and StorageCredential (Iceberg
                           REST's, verbatim); the Engine. And two runtime halves: `FossilError`
                           + `isFossilError` over `problem.gen.ts` (the codes, their data and
                           titles, generated) — the error every package throws, the same object
                           the wasm crates build in Rust — and `within`, the one deadline every
                           host wait and module boot goes through (`/docs/design/failure`).
                           `isCode` is the `area/kind` grammar as a predicate; `referenceTo` is
                           the inverse of `@conn` expansion, held against the Rust in
                           `packages/wasm/tests/reference.test.ts`
  storage/                 how every package reaches storage from a vended credential, over
                           `fossil-storage-wasm`: `mount` (scoped DuckDB secret renewed at
                           expires−5min, refcounted per prefix; Azure lent file by file),
                           and `resolveDocuments`.
                           No host ever signs a URL for fossil
  codemirror-fossil/       the fossil language layer for CodeMirror 6, and it is EXTENSIONS
                           and not an editor, behind ONE door: `fossil(program, options)`.
                           A view plugin pushes the text through `program.update`, then
                           highlighting from `tokenize()` + `semanticTokens()`, squiggles
                           from `diagnostics()` through `@codemirror/lint`, and hover /
                           completion / goto-definition over position-only queries. Code
                           actions stay OUT (the two quick fixes hang off a structured
                           diagnostic the `Diagnostic` wire shape flattens). A package of this name was deleted in
                           `873cbc0` and it is not restored — the old one hard-copied the
                           lexer's DISCRIMINANTS into a TS enum, which was wrong in nine
                           places by the time it went. This one keys on the NAMES the wasm
                           legend ships, and the guard is on the Rust side

docs/                      Next.js + fumadocs, and ALL of the prose: the book, the graph
                           format, and the design argument behind a maintainers' divider.
                           NOT published to npm, and no RECURSIVE CI step reaches it (all are
                           filtered to `./packages/*`); `docs.yml` checks it on PRs and
                           `deploy-docs.yml` publishes it to GitHub Pages.
                           `docs/CLAUDE.md` has the editorial rules

grammar.bnf                the syntax, normative, and ahead of the parser on purpose
catalogue.bnf              which names exist — the `io.` rows, the stdlib rows and the
                           DuckDB type table. Every projection is generated; no Rust or TS
                           states a row twice
```

The `ui/ viewer/ editor/` React family moved to `@kanzo-tech/*`; commit `873cbc0` deleted
those three with `codemirror-fossil/`. **Fossil still ships no UI** — it ships a language
layer, which is the part that was never a UI: `packages/codemirror-fossil` is extensions
over somebody else's editor, and the somebody else is `@kanzo-tech/ui`.

## Style

- Prefer enum dispatch over `Box<dyn Trait>`. Salsa interning needs concrete types.
- `Result<T, E>` with thiserror-style enums in lib crates; `miette` only where a host renders a diagnostic.
- `tracing` for structured logs (not `log`). `RUST_LOG=fossil=debug` is the canonical filter.
- Golden files via `expect-test` (`expect!`/`expect_file!`); `UPDATE_EXPECT=1` rewrites them.
- Doc comments on `pub` items in compiler-core crates.

## Common Tasks

- **Add a new dependency:** add to `[workspace.dependencies]` (workspace-level), then per-crate
  `dep = { workspace = true }`. Run `cargo deny check` to confirm no advisory or license issue.
  Run WASM gate to confirm no transitive WASM-incompat leak. If the dep is foundational, say why
  on the page whose claim depends on it.
- **Add a new crate:** `members = ["crates/*"]` picks it up; copy the shape of an existing
  peer (compiler-core or wasm-shim). Do not
  record the new crate count anywhere — nothing should have one to update.
- **Run the WASM smoke test:** build the nodejs bindgen target, then
  `node crates/fossil-wasm/test-wasm-workspace.js` — the build precondition is in that file's
  header. `crates/fossil-wasm/tests/workspace.rs` is its native mirror and runs on every PR.
- **There are no apps.** fossil is the backend; the views are `@kanzo-tech/graph` and the rest of kanzo-ui. A second viewer here is the thing `apps/playground` was, and it was deleted for it.

## Anti-patterns

- Using `default-features = true` on rudof crates — be explicit about what you opt into; the
  defaults drag in crates that do not build for wasm32.
- Putting compiler logic in `fossil-base` — it is the trait + db substrate, no business logic.
- Skipping the WASM gate locally. CI catches it eventually but the feedback loop is slower.
- Editing a generated file (`packages/wasm/pkg/*`, `crates/fossil-wasm/pkg/*`, or `target/*`). They are regenerated.
