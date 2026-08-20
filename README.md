# Fossil

A typed compiler for RDF graph construction. Surface syntax is a small DSL
(`.fossil`) with bidirectional type checking — forward from input descriptors
(CSVW, or introspected from the source itself), backward from target shapes
(ShEx). It lowers to a typed operator algebra and executes it, producing typed
graphs in [Apache GraphAr](https://graphar.apache.org/) layout: DataFusion runs
the mapping, DuckDB does source introspection and the layout pass.

**Status:** pre-v0.1, in active development. Nothing is published; the surface
is still changing. `playground.kanzo.dev` is not live.

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

That is `apps/docs/programs/hello/hello.fossil` verbatim — one of the programs
the documentation is written against. The surface is the one [`grammar.bnf`](grammar.bnf)
specifies, and the grammar is ahead of the parser on purpose — read a page's `today:`
register before assuming the compiler has arrived there.

## What runs today

`fossil` has four subcommands — `check`, `run`, `providers`, `refs`
(`crates/fossil-cli/src/main.rs`; `--help` on each is authoritative).

The end-to-end one, against the walking-skeleton fixture in `examples/`:

```bash
cargo run --bin fossil -- run examples/hello.fossil --dest file:///tmp/hello
```

`--dest` is required, and it is a URL (`file:///path`, `s3://bucket/prefix`, …).
The run writes an Apache GraphAr dataset there: `vertex/Person.vertex.yml`
declaring the columns, and `vertex/Person/*.parquet` holding the rows in
4,096-row tiles. Inspect it with DuckDB:

```bash
duckdb -c "SELECT * FROM read_parquet('/tmp/hello/vertex/Person/*.parquet')"
```

Five `Person` vertices, subjects `https://example.org/user/1` … `/5`.
`crates/fossil-cli/tests/walking_skeleton.rs` asserts that content — not merely
that files appeared — and is the test that goes red if it stops holding.

`examples/hello.fossil` is a CLI fixture, not a conformance program: it is the
one thing that drives the *binary* end to end and asserts the GraphAr dataset on
disk by content. The language itself is proved by the conformance programs under
`apps/docs/programs/`, which `crates/fossil-engine/tests/programs.rs` compiles
and the documentation transcludes.

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

There is **one** reference, and it is in four pieces that do not overlap:

- [`grammar.bnf`](grammar.bnf) — the language's syntax, normative. The parser implements it; it does
  not describe the parser. Transcluded whole into `/docs/book/grammar`.
- [`apps/docs/`](apps/docs/) — the language (Next.js + fumadocs). `/docs/design` is the argument,
  `/docs/book` teaches it, `/docs/book/typing` is the static semantics — the grammar's sibling — and
  `/docs/characteristics` keeps two registers apart and marks which one each page is in: where
  fossil is going, and what is true today with the file that would go red if it stopped holding.
- [`apps/corpus/`](apps/corpus/) — the artifact: the format, its conventions, and the executable
  guards that make those conventions a contract.
- [`apps/docs/programs/`](apps/docs/programs/) — the eighteen conformance programs. Every program
  the documentation shows is one of these, read off disk at build time and never retyped into prose.

Plus [`CONTRIBUTING.md`](CONTRIBUTING.md) for the dev cycle and commit policy, and
[`CLAUDE.md`](CLAUDE.md) for project memory.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) OR [MIT](LICENSE-MIT) at your option.
