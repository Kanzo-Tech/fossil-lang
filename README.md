# Fossil

A statically typed mapping language that turns tables and RDF into a property
graph. Surface syntax is a small DSL (`.fossil`) with bidirectional type
checking — forward from input descriptors (introspected from the source
itself), backward from target shapes (ShEx). Every vertex has one static type,
the shape its mapping declares. It lowers to a typed operator algebra and
executes it, writing a
`fossil/1` corpus: one Parquet per vertex type and per relation, and a small
`fossil.json` over them. DataFusion runs the mapping, the layout pass places the
whole graph and numbers it in Hilbert order, and any SQL engine that reads Parquet
reads the result — `/docs/design/corpus` is the argument.

**Status:** alpha. The `@fossil-lang/*` packages are published to npm under the
`alpha` dist-tag, one version per git tag; the surface is still changing.

## Quick look

```bash
cargo check --workspace
cargo test --workspace
cargo xtask wasm-check   # the WASM gate CI runs; xtask derives the crate set
```

The WASM gate has no hand-maintained crate list. `crates/xtask` walks the
dependency closure of the workspace's cdylib crates and checks exactly that
set against `wasm32-unknown-unknown`, which is what
`.github/workflows/ci.yml` runs.

## The language

A mapping declares how sources become a typed graph. Shapes come from a ShEx
document, the source is a constructor, and every reference is qualified by the
binding it came from:

```fossil
type { Person } := io.shex("hello.shex")

User := io.csv("data/people.csv")

People : Person from User
    @subject = "https://shop.example/person/{User.id}"
    name     = User.name
```

That is `docs/programs/hello/hello.fossil` verbatim — one of the programs
the documentation is written against. The surface is the one [`grammar.bnf`](grammar.bnf)
specifies, and the grammar is ahead of the parser on purpose — a production the parser
does not implement yet is work outstanding, not an error in the file.

## What runs

The executor — `fossil_df::Executor`, published as `@fossil-lang/executor` — is
the one host that runs a program and writes its corpus, in the browser and in
Node. There is no native CLI; `/docs/design/discarded` says why and what would
bring one back.

The walking skeleton is `examples/hello.fossil`: run through the executor it
writes `fossil.json` and `vertex/Person.parquet`, five `Person` vertices with
subjects `https://example.org/user/1` … `/5`. Inspect a written corpus with
DuckDB:

```bash
duckdb -c "SELECT * FROM read_parquet('<dest>/vertex/Person.parquet')"
```

`crates/fossil-df/tests/walking_skeleton.rs` asserts that content — not merely
that files appeared — and is the test that goes red if it stops holding. The
language itself is proved by the conformance programs under `docs/programs/`,
which `crates/fossil-df/tests/programs.rs` compiles and runs and the
documentation transcludes.

## Foundations

- **Operator algebra** (and the rest of the reading, named, is at `/docs/design/prior-art`):
  typed extension of [Min Oo & Hartig — *An Algebraic
  Foundation for Knowledge Graph Construction*](https://arxiv.org/abs/2503.10385)
  (ESWC 2025 Best Research Award). Fossil's MIR is a conservative typed elaboration
  of their algebra; the untyped projection is operationally equivalent.
- **Shape semantics**: target type checking via [`rudof`](https://github.com/rudof-project/rudof),
  the WESO group's Rust ShEx implementation (Universidad de Oviedo, José Emilio Labra Gayo).
- **Compiler architecture**: Salsa-based incremental queries (rust-analyzer / ty pattern),
  lossless CST via rowan, LSP from day 1.

## Documentation

There is **one** reference, in three pieces:

- [`grammar.bnf`](grammar.bnf) — the language's syntax, normative. The parser implements it; it does
  not describe the parser. Transcluded whole into `/docs/book/grammar`.
- [`docs/`](docs/) — all of the prose (Next.js + fumadocs). `/docs/book/getting-started`
  teaches the language, `/docs/format` specifies the corpus, and behind a maintainers' divider `/docs/design`
  is the argument for why any of it is shaped this way. A page describing something not yet built
  says so in a `direction:` field rather than in its tone; everything else describes what is there.
  Every push to `main` publishes it to <https://kanzo-tech.github.io/fossil-lang/>.
- [`packages/corpus/`](packages/corpus/) — the reader, `@fossil-lang/corpus`, and beside it the
  artifact's contract, executable: the guards that make a convention checkable and a conformance
  corpus that satisfies every one of them. Copy `guards/`
  somewhere else and it runs — `node` and a `duckdb` binary, no install, no build.
- [`docs/programs/`](docs/programs/) — the conformance programs. Every program the
  documentation shows is one of these, read off disk at build time and never retyped into prose.
  No count here: `crates/fossil-df/tests/programs.rs` walks the directory, and the number this
  line used to carry was five behind it.

Plus [`CONTRIBUTING.md`](CONTRIBUTING.md) for the dev cycle and commit policy, and
[`CLAUDE.md`](CLAUDE.md) for project memory.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) OR [MIT](LICENSE-MIT) at your option.
