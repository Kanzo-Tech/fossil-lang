# Fossil — Claude Code project memory

This file is loaded into every Claude Code session in this repo.
Keep it under 200 lines, rules-not-context.

## Read These First

- `grammar.bnf` — the syntax, normative, and ahead of the parser on purpose. A production
  written here and absent from `crates/fossil-syntax` is work outstanding, not an error in
  the file.
- `corpus.bnf` — WHAT A CORPUS IS MADE OF: the columns the writer emits and what each
  one IS. The third data file, and the newest. A reader that needs «the writer's columns»
  names the ROLES it means rather than the names — two readers used to write the names and
  meant different questions by them. `cargo xtask corpus` generates the Rust projection;
  `crates/xtask/tests/corpus_generated.rs` is the `--check` as a test.
- `catalogue.bnf` — WHICH NAMES EXIST, both halves: the `io.` constructors and every stdlib
  function with its signature and lowering. Generated from, not compared against — seven files
  come out of `cargo xtask catalogue` and no Rust states a row a second time. Adding a
  function is a line here. `CONTRIBUTING.md` has the seven and the round-trip guard.
- `docs/` — the whole of the documentation, and there is no second site.
  `/docs/book/getting-started` teaches the language and `/docs/format` specifies the corpus;
  behind a maintainers' divider,
  `/docs/design` is where an argument lives, with `design/discarded` for every rejected
  alternative and what would bring it back, and `design/prior-art` for every source named.
- `packages/corpus/` — the artifact's contract, executable: `guards/`, `conformance/` and
  `integration/`. Its prose is `/docs/format`; the server component that renders `guards.mjs`
  lives on the docs side and reads across, so renaming it breaks the docs build on purpose. `integration/` is the one directory here that needs `pnpm install`, and
  `pnpm test:integration` is the only script that reaches it.
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
  `tower-lsp` (unmaintained ~3 years; use `lsp-server`, as all three reference implementations do),
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
  (`fossil_df::Executor`, the one write host), must keep producing a readable `fossil/1`
  corpus — 5 `Person` vertices, asserted by content read back with DuckDB, not existence.
  `crates/fossil-df/tests/walking_skeleton.rs` is the test that goes red. A refactor that
  breaks it for >3 days is reverted and broken into smaller steps.

## Stack Pins

| Component | Version | Notes |
|-----------|---------|-------|
| Rust toolchain | 1.90 | per `rust-toolchain.toml`; bumped from 1.85 to unblock wasm-bindgen-cli 0.2.120 install |
| salsa | 0.28.5 + `accumulator` feature | incremental query framework; ≥0.28.5 for RUSTSEC-2026-0308. 0.28 returns by reference by default, so every tracked item that predates it names `returns(clone)` or `returns(copy)`, the 0.26 default |
| rowan | 0.16 | lossless CST |
| logos | 0.16 | lexer |
| miette | 7.6 | diagnostics |
| duckdb | 1.10502 (`features = ["bundled"]`) | native execution |
| wasm-bindgen | =0.2.120 | exact pin; CLI must match |
| wasm-opt (binaryen) | 116 via `cargo install wasm-opt@0.116.1` | NOT apt (ubuntu ships binaryen 108, whose wasm-opt corrupts wasm-bindgen's externref table → `Table.grow(): failed to grow table` instantiating a wasm-bindgen module on Node 20, binaryen #4711; 116 fixes it). `packages/executor/scripts/build-wasm.sh` passes the six wasm32 default features (bulk-memory, sign-ext, mutable-globals, nontrapping-fptoint, reference-types, multivalue — Rust 1.87/LLVM 20). NOT `-all` → no gc/typed-funcref, which break instantiation |
| lsp-server | 0.7 | NOT tower-lsp (unmaintained) |
| arrow + parquet | 58 | the corpus writer, `fossil_df::write` (through `datafusion`'s re-export) |
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
  fossil-locator/          the ONE rule turning a written reference into something a reader can
                           open — `@conn`, scheme, absolute, else the program's directory; never
                           the cwd. It was a module of `fossil-base` and is not one now: a
                           substrate «doesn't know about file paths». It depends on NOTHING and
                           `fossil-base` does not depend on IT — each of its readers
                           (`fossil-hir`, `-introspect`, `-df`, `-lineage`) takes it direct
  fossil-syntax/           lossless CST + parser
  fossil-hir/              types + name resolution + bidirectional checker + the stdlib
                           catalog. `stdlib.rs` owns the TYPES a row is written in; the ROWS
                           are `catalogue.bnf` and `stdlib/generated.rs` is what
                           `cargo xtask catalogue` makes of them. Adding a function is a
                           line in the `.bnf` and touches no Rust
  fossil-mir/              typed operator algebra; `src/op.rs` is the operator enum
  fossil-shex/             ShExDescriptor over `shex_ast` — backward target-shape checking
  fossil-descriptors-{input,output}/   trait + impls (ShEx, inferred)
  fossil-storage/          a vended `StorageCredential` (Iceberg REST's) → a scoped DuckDB secret,
                           an Azure SAS lend, or (feature `object-store`) an `object_store` store
                           routed by longest prefix and renewed at expires−5min — the one IO path
                           of DataFusion and of every byte fossil reads or writes. `js`: a JS
                           `Host` as its `Host`. The one renderer of a `CREATE SECRET`, native
                           hosts' included
  fossil-storage-wasm/     that, exposed to JS for `@fossil-lang/storage`
  fossil-lineage/          source lineage + provider introspection, projected onto the wire
  fossil-sinks/            the `fossil/1` format: `fossil.json`'s structs (serde + schemars;
                           `fossil.schema.json` beside them is what a reader in another
                           language checks against), `FOSSIL_FORMAT`, and `generated.rs` —
                           the writer's column table from `corpus.bnf`. It byte-writes
                           nothing: `arrow-schema` for the types, `serde_json` to emit
  fossil-df/               DataFusion backend for the property-graph MIR, the one corpus
                           writer (`write`: a Parquet per table, `fossil.json`
                           last), and `Executor` — the whole run a host drives, under the
                           2 GiB pool in `memory.rs` that refuses as `run/over-budget`
  fossil-introspect/       a host job and not a compiler one: `DESCRIBE` each source's columns,
                           and the payload that authenticates one. `fossil-lsp` calls it
                           before the compile. It links `DuckDB` on a normal edge, and
                           it is not the only crate that does — `cargo tree -e normal -i duckdb
                           --workspace` is the list, and `crates/xtask/tests/engine_reach.rs`
                           holds it against `deny.toml`. Native by that edge, without a
                           tripwire of its own. Which sources it
                           DESCRIBEs is `fossil_lineage::program_sources` — the list the
                           browser's `sources()` returns — so it links the compiler front-end,
                           as every host does
  fossil-mem-probe/        `FOSSIL_MEM_PROBE` — peak RSS + elapsed seconds per phase of a
                           run. Depends on NOTHING
  fossil-graph-schema/     the canonical graph-schema — the shared substrate contract — and the
                           error catalogue: `Problem`, `Failure`, and (feature `js`) the one
                           function every wasm crate throws a failure through
  fossil-ide/              hover, completion, goto-def + the symbol/prefix/workspace indexes
  fossil-lsp/              LSP server via lsp-server  [NATIVE-ONLY]
  fossil-wasm/             WASM host shim (FossilWorkspace API + the tokenizer the editor reuses)
  fossil-df-wasm/          `fossil_df::Executor` exposed to JS — the only host that writes a
                           corpus, in the browser and in Node. There is no native CLI: it
                           was deleted on 2026-09-30 (`/docs/design/discarded` says what
                           brings it back)
  xtask/                   repo automation. Four commands: `wasm-check` derives the wasm32
                           subset from the cdylib closure, and `catalogue [--check]`,
                           `corpus [--check]` and `problem [--check]` regenerate every
                           projection of their source — `catalogue.bnf`, `corpus.bnf`, and
                           `fossil-graph-schema/problem.schema.json` into `problem.gen.ts`.
                           One generator loop behind all three, so the `--check` semantics
                           cannot drift between them

packages/                  npm-published @fossil-lang/* family (pnpm workspace)
  wasm/                    wraps fossil-wasm build outputs (.js + .wasm + .d.ts)
  corpus/                  the reader, and it is TypeScript and nothing else: `fossil.json`
                           through `JSON.parse`: `open` attaches one view per table on the host's
                           DuckDB and answers its `close`; `mapping` answers the corpus's RDF
                           meaning as RML 2.0. `src/index.ts` is the surface. It links no engine
                           and loads no WASM. Beside `src/` sits the contract it fulfils — `guards/`,
                           `conformance/` and `integration/` — outside `files`, so npm never
                           sees it. `tests/manifest.test.ts` holds `src/manifest.ts` against
                           `fossil-sinks/fossil.schema.json`; `integration/round-trip.test.ts`
                           runs a program through the executor and reads the corpus back over
                           HTTP. `test` stays `tests/` alone because the release gate runs it
                           without `duckdb`; the contract is `test:contract` (`corpus.yml`) and
                           `test:integration` (`pnpm-ci.yml`). It was `@fossil-lang/graph` and it
                           is not a graph: the thing that draws one is `@kanzo-tech/graph`, the
                           one view layer, in kanzo-ui
  executor/                the datafusion-wasm executor — the one writer of a corpus. A run
                           answers `{ dest, dropped }`; what it wrote is `<dest>fossil.json`
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
                           `read` (an `object_store` GET), and `resolveDocuments`.
                           No host ever signs a URL for fossil
  codemirror-fossil/       the fossil language layer for CodeMirror 6, and it is EXTENSIONS
                           and not an editor. FIVE of them, not two: highlighting from
                           `tokenize()` + `tokenKinds()`, squiggles from `check()` through
                           `@codemirror/lint`, and hover / completion / goto-definition
                           over the three position queries `FossilWorkspace` grew. The
                           two that stay OUT are semantic tokens (only the native
                           `fossil-lsp` serves them) and code actions (the two quick fixes hang
                           off a structured diagnostic the `CheckRow` wire shape
                           flattens); `src/index.ts` says so. A package of this name was deleted in
                           `873cbc0` and it is not restored — the old one hard-copied the
                           lexer's DISCRIMINANTS into a TS enum, which was wrong in nine
                           places by the time it went. This one keys on the NAMES the wasm
                           legend ships, and the guard is on the Rust side
  introspect/              source-binding schema introspection (the one home; `fossil-introspect`
                           is the Rust sibling). Their agreement is ENFORCED, not asserted:
                           `packages/introspect/tests/rust-parity.test.ts` derives the reader
                           arms, the option keyword and the type table out of
                           `crates/fossil-introspect/src/lib.rs` and fails on drift. It is a
                           **pnpm** test — editing that Rust turns it red and `cargo test` will
                           not tell you.

docs/                      Next.js + fumadocs, and ALL of the prose: the book, the corpus
                           format, and the design argument behind a maintainers' divider.
                           NOT published to npm, and no RECURSIVE CI step reaches it (all are
                           filtered to `./packages/*`); `docs.yml` checks it on PRs and
                           `deploy-docs.yml` publishes it to GitHub Pages.
                           `docs/CLAUDE.md` has the editorial rules

grammar.bnf                the syntax, normative, and ahead of the parser on purpose
catalogue.bnf              which names exist — the `io.` rows and the stdlib rows. The
                           source of seven generated files; no Rust states a row twice
corpus.bnf                 what a corpus is made of — the vertex and edge columns the writer
                           emits and the ROLE of each. The source of one generated file,
                           `fossil-sinks/src/generated.rs`. File names are NOT here: they are
                           `fossil.json`'s `path`, and `fossil-sinks::manifest` owns them
tests/wasm_parity/         the manual DuckDB-WASM cross-engine parity harness
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
  peer (compiler-core, native-only with the `wasm32` cfg-tripwire, or wasm-shim). Do not
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
