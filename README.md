# Fossil

A typed compiler for RDF graph construction. Surface syntax is a small DSL
(`.fossil`) with bidirectional type checking — forward from input descriptors
(CSVW, JSON Schema), backward from target shapes (ShEx). Compiles to DuckDB SQL
that produces typed graphs in [Apache GraphAr](https://graphar.apache.org/) layout.

**Status:** pre-v0.1, in active development. Phase 0 (workspace genesis) complete;
Phase 1 (walking-skeleton "Hello, Fossil") next. See [`.planning/ROADMAP.md`](.planning/ROADMAP.md)
for the 10-phase plan toward `playground.kanzo.dev` v0.1 (the public Phase 9 deliverable —
not yet live).

## Quick look

```bash
cargo check --workspace
cargo test --workspace
```

WASM target (the playground compiles via this gate from commit #1):

```bash
cargo check --target wasm32-unknown-unknown \
    -p fossil-base -p fossil-syntax -p fossil-hir \
    -p fossil-mir -p fossil-codegen -p fossil-wasm
```

## Foundations

- **Operator algebra**: typed extension of [Min Oo & Hartig — *An Algebraic
  Foundation for Knowledge Graph Construction*](https://arxiv.org/abs/2503.10385)
  (ESWC 2025 Best Research Award). Fossil's MIR is a conservative typed elaboration
  of their algebra; the untyped projection is operationally equivalent.
- **Shape semantics**: target type checking via [`rudof`](https://github.com/rudof-project/rudof),
  the WESO group's Rust ShEx implementation (Universidad de Oviedo, José Emilio Labra Gayo).
- **Compiler architecture**: Salsa-based incremental queries (rust-analyzer / ty pattern),
  lossless CST via rowan, LSP from day 1.

## Documentation

- [`.planning/PROJECT.md`](.planning/PROJECT.md) — project context, requirements, constraints
- [`.planning/ROADMAP.md`](.planning/ROADMAP.md) — 10 phases toward v0.1
- [`.planning/STATE.md`](.planning/STATE.md) — current focus
- [`decisions/`](decisions/) — ADRs (Nygard format)
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — dev cycle, ADR ritual, commit policy
- [`CLAUDE.md`](CLAUDE.md) — Claude Code project memory (rules-not-context)

The five design docs in repo root (`architecture.md`, `grammar.bnf`, `type-system.md`,
`operator-algebra.md`, `stdlib.md`) are the original v0.1 design corpus. Some specifics
have been superseded by research synthesis — when in doubt, ADRs win.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) OR [MIT](LICENSE-MIT) at your option.
