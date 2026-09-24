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
  meant different questions by them. `cargo xtask corpus` generates the Rust and TypeScript
  projections; `crates/xtask/tests/corpus_generated.rs` is the `--check` as a test.
- `catalogue.bnf` — WHICH NAMES EXIST, both halves: the `io.` constructors and every stdlib
  function with its signature and lowering. Generated from, not compared against — six files
  come out of `cargo xtask catalogue` and no Rust states a row a second time. Adding a
  function is a line here. `CONTRIBUTING.md` has the six and the round-trip guard.
- `apps/docs/` — the whole of the documentation, and there is no second site.
  `/docs/book/getting-started` teaches the language and `/docs/format` specifies the corpus;
  behind a maintainers' divider,
  `/docs/design` is where an argument lives, with `design/discarded` for every rejected
  alternative and what would bring it back, and `design/prior-art` for every source named.
- `apps/corpus/` — the artifact's contract, executable: `guards/`, `conformance/` and
  `integration/`. Its prose is `/docs/format`; the two server components that render `guards.mjs`
  and `vectors.json` live on the docs side and read across, so renaming either file breaks the
  docs build on purpose. `integration/` is the one directory here that needs `pnpm install`, and
  `pnpm test:integration` is the only script that reaches it.
- `apps/docs/programs/` — the conformance programs. Documentation transcludes them; nothing
  retypes a program into prose. No number here: `crates/fossil-cli/tests/programs.rs` walks
  the directory, and the count in this line was already wrong.
- `crates/` — the crate list. There is no number to quote; `cargo xtask wasm-check` prints
  the wasm32 subset it derived from the dependency graph.
- `apps/docs/CLAUDE.md` — the editorial rules for the docs app: which pages declare a
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
- **The batch pass does not link the compiler substrate.** `fossil-layout` must reach `salsa`
  over no normal edge. It reached it over four branches until `39d0fb8`: one `use` of a 37-line
  Parquet encoder the executor never called put a compiler front-end and a query engine inside
  the closure of a pass that resolves no name and holds no database. The DEV edge stays — an
  example measures the tiling against the baseline encoder in `fossil_df::files`, and an example
  links dev-dependencies. **This bullet is the policy and the only copy of it:**
  `crates/xtask/tests/substrate_reach.rs` reads the crate name out of THIS line, derives the
  linker set from `cargo metadata`, and prints the whole table when it goes red — so neither
  half is ever written down twice. It does NOT keep the substrate out of the wasm payload and
  never claimed to: two cdylib roots take the compiler directly, and the gate's crate set did
  not shrink when this edge moved.
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
  producing a readable corpus — 5 `Person` vertices, asserted by content, not existence. It said
  "a valid GraphAr dataset", and that is not the claim: fossil borrows GraphAr's manifest field
  names and stops where the spec stops specifying, and a fossil corpus is not openable by GraphAr's
  own reader (its `TypeNameToDataType` throws on `dense_id`'s `uint32`). See
  `/docs/design/corpus` for the boundary and the nine measured divergences.
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
  fossil-locator/          the ONE rule turning a written reference into something a reader can
                           open — `@conn`, scheme, absolute, else the program's directory; never
                           the cwd. It was a module of `fossil-base` and is not one now: a
                           substrate «doesn't know about file paths». It depends on NOTHING and
                           `fossil-base` does not depend on IT — each of its five readers
                           (`fossil-hir`, `-cli`, `-introspect`, `-df`, `-lsp`) takes it direct
  fossil-syntax/           lossless CST + parser
  fossil-hir/              types + name resolution + bidirectional checker + the stdlib
                           catalog. `stdlib.rs` owns the TYPES a row is written in; the ROWS
                           are `catalogue.bnf` and `stdlib/generated.rs` is what
                           `cargo xtask catalogue` makes of them. Adding a function is a
                           line in the `.bnf` and touches no Rust
  fossil-mir/              typed operator algebra; `src/op.rs` is the operator enum
  fossil-shex/             ShExDescriptor over `shex_ast` — backward target-shape checking
  fossil-descriptors-{input,output}/   trait + impls (ShEx, inferred)
  fossil-resolver/         host-injected cloud path resolution (s3://, az://)  [NATIVE-ONLY]
  fossil-lineage/          source lineage + provider introspection, projected onto the wire
  fossil-sinks/            the canonical GraphAr manifest model, plus `generated.rs` — the
                           writer's column table from `corpus.bnf`, which lives here because
                           this crate is the junta: in the closure of both the writer and the
                           reader, so no consumer grows an edge to reach it.
                           It byte-writes nothing —
                           `arrow-schema` for the types, `serde_yaml_ng` to emit, and
                           `fossil-policy` because a declared bound is part of what the
                           artefact says about itself
  fossil-df/               DataFusion backend for the property-graph MIR
  fossil-policy/           the privacy policy document a corpus is verified against — ODRL for
                           structure, DPV for classification, and a fossil profile for the terms
                           neither vocabulary has. A SECOND document and not an annotation on the
                           shape: a shape is vocabulary and travels, a classification is
                           jurisdiction and does not
  fossil-kanon/            Mondrian strict multidimensional k-anonymity over Arrow arrays,
                           WASM-clean. Two separable halves: `anonymize` DERIVES a
                           generalisation, `verify::assess` CHECKS a published one needing no
                           hierarchy and no sensitive column — so an auditor with the Parquet and
                           no entitlement to the diagnosis column still gets the answer
  fossil-introspect/       a host job and not a compiler one: `DESCRIBE` each source's columns,
                           and the `--creds-stdin` payload that authenticates one. `fossil-cli`
                           calls it before the compile. It links `DuckDB` on a normal edge, and
                           it is not the only crate that does — `cargo tree -e normal -i duckdb
                           --workspace` is the list, and `crates/xtask/tests/engine_reach.rs`
                           holds it against `deny.toml`. Native by that edge and by
                           `fossil-resolver`, without a tripwire of its own. It links `salsa`
                           too, through `fossil-base`, and that is NOT what `39d0fb8` cut: it
                           names no `Db`, no query and no diagnostic, only the substrate's two
                           salsa-free halves that a host HAS to name — `System::descriptors`,
                           the cache it fills, and the `io.` rows the scrape's alternation and
                           its reader choice come from. Cutting that edge is a split of
                           `fossil-base` and nothing in here, which is why the substrate bullet
                           does not name this crate
  fossil-layout/           the layout post-pass — Louvain + Morton over Parquet through
                           arrow-rs. It links no engine: `DuckDB` and `fossil-df` are both
                           dev-dependencies, the second since `TileWriter` became
                           `fossil-tile-writer` and stopped dragging `DataFusion` and
                           `salsa` in behind it. `crates/xtask/tests/engine_reach.rs`
                           derives the real linkers and is what made that edge visible. It
                           COMPILES for wasm32 and declares it with `[package.metadata.fossil]
                           wasm = true`, which is what puts it in the gate closure. It was
                           `fossil-runtime`, and it is not a runtime: the crate IS the pass
  fossil-tile-writer/      the row-group container a corpus's payload is written through —
                           one tile, one row group, and the only byte-writer of tiles in the
                           tree. A leaf over `arrow` and `parquet`. It was a struct in
                           `fossil_df::files` that `fossil-df` never called, and the layout
                           pass's one `use` of it is what put `salsa` and `DataFusion` under
                           a pass that resolves no name. NOT in `fossil-sinks`: that crate
                           takes `arrow-schema` alone and declares the tiling without
                           byte-writing it
  fossil-mem-probe/        `FOSSIL_MEM_PROBE` — peak RSS + elapsed seconds per phase of a
                           write. Depends on NOTHING; both halves of the write path
                           (fossil-df, fossil-layout) report through it
  fossil-graph-schema/     the canonical graph-schema — the shared substrate contract
  fossil-graph/            the typed verb surface over a GraphAr corpus. It EMITS SQL in
                           DuckDB's dialect and links no engine to run it (WASM-clean)
  fossil-mcp/              that same verb surface as a native server-side service
  fossil-ide/              hover, completion, goto-def + the symbol/prefix/workspace indexes
  fossil-cli/              fossil's native HOST *and* the binary over it: `src/host.rs` is the
                           `System` the compiler runs against, the shape documents a program
                           names, and the compile→run pipeline; `src/main.rs` is
                           `fossil check/run/providers/refs` and does the rendering. It was
                           `fossil-engine` plus a shell, and the library half had exactly one
                           consumer — this one. `fossil-lsp` and `fossil-wasm` keep their hosts
                           inside themselves too: three hosts, three crates  [NATIVE-ONLY]
  fossil-lsp/              LSP server via lsp-server  [NATIVE-ONLY]
  fossil-wasm/             WASM host shim (FossilPlayground API + the tokenizer the editor reuses)
  fossil-df-wasm/          the fossil-df executor exposed to JS
  fossil-graph-wasm/       wasm-bindgen binding for the fossil-graph verb surface
  xtask/                   repo automation. Three commands: `wasm-check` derives the wasm32
                           subset from the cdylib closure, and `catalogue [--check]` and
                           `corpus [--check]` regenerate every projection of the data file
                           each is named after. One generator loop behind both, so the
                           `--check` semantics cannot drift between them

packages/                  npm-published @fossil-lang/* family (pnpm workspace)
  wasm/                    wraps fossil-wasm build outputs (.js + .wasm + .d.ts)
  corpus/                  ONE door over one manifest, and every part of it needs the gitignored
                           `pkg/`. The addressing is no longer a second implementation: it is
                           `fossil_graph::plan` compiled to wasm32, and the package is a
                           binding over it. The subpath `@fossil-lang/corpus/address` existed so a
                           caller could address a corpus WITHOUT wasm, and it is deleted with its
                           standalone test — composing a URL now costs loading the module, which is
                           the price of there being one reader instead of two.
                           It was `@fossil-lang/graph` and it is not a graph: its door is
                           `open` and the thing that DOES draw one, `@kanzo-tech/graph`,
                           sits beside it in the playground's `package.json`. The Rust crates
                           keep their names — `fossil-graph` IS a verb surface over a property
                           graph, and a crate name is not in npm's import space.
                           ONE name at three depths, because the capability the caller brings
                           decides how deep the answer is: `{ query }` is the whole corpus,
                           `{ readText }` the manifests alone, `{ manifestFiles }` no request at
                           all. `resolveCorpus` is gone rather than renamed and all three are
                           ASYNC; `corpus.addressing` is what the first already resolved. The
                           boot is the `wasmUrl` option on every depth and is internal otherwise
  draw/                    the half of drawing a corpus that is NOT a renderer — no canvas, no
                           GPU, no camera, and fossil still ships no viewer. What a corpus is
                           drawn WITH (the `channels:` block, its three states, the derivation
                           for a corpus that declares nothing) and what is already LOADED (the
                           frames a door answered, and the interim assembled from them). NOT part
                           of `corpus/`: a corpus has no opinion about which column deserves a
                           histogram, and every part of `corpus/` static-imports the wasm while
                           nothing here needs one
  executor/                datafusion-wasm query executor, and the manifest wire mirror, because
                           a run HANDS THAT BACK — except `Channel`/`Scale`, in `types/` because
                           `draw/` reads one and 22 MB of wasm is the wrong price for an interface
  types/                   shared TS types (SourceHost, the one host contract; FossilTheme; the
                           `channels:` wire shape — zero runtime)
  resolvers/               default + mock + public-HTTP resolvers, over a contract of their
                           own until they become SourceHosts
  codemirror-fossil/       the fossil language layer for CodeMirror 6, and it is EXTENSIONS
                           and not an editor. FIVE of them, not two: highlighting from
                           `tokenize()` + `tokenKinds()`, squiggles from `check()` through
                           `@codemirror/lint`, and hover / completion / goto-definition
                           over the three position queries `FossilPlayground` grew. The
                           two that stay OUT are semantic tokens (they come back only
                           over the Worker) and code actions (the two quick fixes hang
                           off a structured diagnostic the `CheckRow` wire shape
                           flattens); `src/index.ts` says so. A package of this name was deleted in
                           `873cbc0` and it is not restored — the old one hard-copied the
                           lexer's DISCRIMINANTS into a TS enum, which was wrong in nine
                           places by the time it went. This one keys on the NAMES the wasm
                           legend ships, and the guard is on the Rust side
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
  docs/                    Next.js + fumadocs, and ALL of the prose: the book, the corpus
                           format, and the design argument behind a maintainers' divider.
                           `apps/docs/CLAUDE.md` has the editorial rules
  corpus/                  NOT a site — it was one, and it is now `docs/content/docs/format/`.
                           What is left executes: the guards, the conformance corpus, a reader
                           that shares no code with them, and `integration/` — the four vitest
                           suites that need the `duckdb` BINARY, which is why they are here and
                           not in `packages/corpus/tests/`. They were there, reaching backwards
                           over `../../../apps/corpus/guards/`, and a published package that
                           imports out of a private app is a package the release gate's
                           `--filter "./packages/*"` cannot isolate — `v0.3.0-alpha.4` is what
                           that cost. `pnpm-ci.yml` runs them; it is the job that has both a
                           `duckdb` binary and `packages/corpus/pkg/`
  playground/              the architectural claim, clickable: check → run → query in one
                           browser tab, no server. It imports `examples/hello.fossil` rather
                           than copying it, and produces the same five subjects the walking
                           skeleton asserts natively. It runs the REAL layout pass — Louvain
                           and Morton through `fossil-layout` over a `MemoryFs` — so the corpus
                           the tab writes is byte-identical to the one `fossil run` writes. Its
                           editor is `@kanzo-tech/ui`'s `CodeEditor` with
                           `packages/codemirror-fossil` inside it, and its canvas IS
                           `@kanzo-tech/graph` — this line said it was not, and the tree
                           won. What the app does NOT take is `@kanzo-tech/mosaic`: that
                           subpath is an OPTIONAL peer, and taking it pulls
                           `@uwdata/mosaic-core`, which hard-depends on a SECOND
                           DuckDB-WASM. `src/tiles.ts` implements `BoundedSource` over the
                           app's own connection instead; `pnpm --filter
                           @fossil-lang/playground... build` emitting one `duckdb-*.wasm`
                           asset is the condition that keeps it true

grammar.bnf                the syntax, normative, and ahead of the parser on purpose
catalogue.bnf              which names exist — the `io.` rows and the stdlib rows. The
                           source of six generated files; no Rust states a row twice
corpus.bnf                 what a corpus is made of — the payload and adjacency columns and
                           the ROLE of each. The source of two generated files, one Rust and
                           one TypeScript. Prefixes and file names are NOT here: those are
                           addressing, and `fossil-graph`'s plan already owns them
tests/wasm_parity/         the manual DuckDB-WASM cross-engine parity harness
```

The `ui/ viewer/ editor/` React family moved to `@kanzo-tech/*`; commit `873cbc0` deleted
those three with `codemirror-fossil/`. **Fossil still ships no UI** — it ships a language
layer, which is the part that was never a UI: `packages/codemirror-fossil` is extensions
over somebody else's editor, and the somebody else is `@kanzo-tech/ui`.

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
