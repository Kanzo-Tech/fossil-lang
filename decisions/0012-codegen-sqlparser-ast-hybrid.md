# ADR 0012: Build codegen SELECT bodies via sqlparser AST, hand-wrap COPY

**Date:** 2026-05-20
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** `.planning/phases/04-mir-algebra-rewriting-complete-codegen/04-RESEARCH.md`
(§"sqlparser 0.59 AST Construction", §"Per-operator SQL mapping", Pitfall 1);
`operator-algebra.md` §6

## Context

Codegen (`fossil-codegen`) must turn the typed MIR operator algebra into
byte-stable, well-quoted `DuckDB` SQL for the 11-operator algebra (CORE-10).
Phase 1 hand-`format!`'d a trivial `CREATE VIEW` + `COPY (SELECT ...)` script
and exercised zero `sqlparser` API surface — fine for the single hello.fossil
shape, but fragile as the operator set grows: quoting (identifiers, string
literals), `DISTINCT ON` / `EXCLUDE` spelling, `UNION` nesting, and predicate
rendering are all easy to get subtly wrong with string templates, and the
SC#1 byte-stable snapshot corpus is far easier to keep stable when the
structured SQL is rendered from a validated AST rather than concatenated.

`sqlparser` 0.59 is parser-first: there is no builder API, so hand-constructing
AST nodes means populating large `#[non_exhaustive]`-shaped structs
(`ast::Select` alone has 23 fields, several of which shift across minor
releases). Worse, the terminal statement we emit — `COPY (...) TO 'output.parquet'
(FORMAT PARQUET)` — does **not** round-trip through `sqlparser` 0.59:
`Statement::Copy::to_string()` drops the parenthesised `(FORMAT PARQUET)`
options form (Pitfall 1, already encoded in the Phase 1 `sql.rs` doc-comment).
So a pure-AST strategy is impossible for COPY, and a pure-string strategy is
fragile for everything else.

We also must keep `fossil-codegen` WASM-clean (SC#5): codegen emits SQL **text**
only; `duckdb` (native, bundled) lives in `fossil-runtime`. `sqlparser` 0.59
pulls only `log` + `recursive` (→ `stacker`), all WASM-clean, so the AST usage
adds zero native-only transitive deps.

## Decision

We will build the inner `SELECT` bodies of the single-input operators
(`Project` / `Extend` / `Rename` / `Filter` / `Distinct` / `Union` / `Empty`)
via `sqlparser::ast` construction — `Select` / `SetExpr` / `Query` for the
structured parts (projection list, `FROM`, `WHERE`, `DISTINCT`, `UNION`,
`EXCLUDE`) — and render them with `.to_string()` (`Display`). The 23-field
`ast::Select` literal is centralised in a single `base_select` helper
(`crates/fossil-codegen/src/ast.rs`) so a future `sqlparser` bump that
adds/renames a field fails to compile in exactly one place; the verified 0.59
field set is documented in the module doc-comment.

Leaf **expression** fragments (the `||` concats, `<op>` comparisons) keep
flowing through the existing string path in `sql::render_expr` and are
re-parsed into a `sqlparser::ast::Expr` via `Parser::parse_expr` before being
spliced into the structured AST — the hybrid the research recommends (AST where
it buys stable quoting/structure, string fragments where hand-construction is
more verbose than valuable). The terminal `COPY` statement stays a
hand-formatted template in `sql.rs` (Pitfall 1).

Each op is referenced by its downstream consumers through a per-index relation
reference: a bare view name for `Source`, or a nested subquery
`(<body>) AS step_<i>` for the SELECT ops (subquery chosen over a `WITH` CTE
chain for self-containment and snapshot stability). A column qualifier
(`step_<i>` alias, distinct from the FROM-clause subquery text) is threaded
separately so a bare `ColRef` prefixes the alias, not the whole subquery. A
buffered `Extend` feeding a `TripleEmit` collapses inline into the COPY's inner
SELECT over the source view, keeping `fossil compile examples/hello.fossil`
byte-identical with Phase 1.

## Consequences

- **Positive:** byte-stable, well-quoted SQL with low risk of quoting/precedence
  bugs as the operator set grows; the SC#1 snapshot corpus stays stable. No
  `duckdb` in `fossil-codegen` — the crate stays WASM-clean (SC#5: zero
  tokio/mio/reqwest paths on wasm32). The single `base_select` literal localises
  `sqlparser`-field-drift risk to one site.
- **Negative / risk:** hand-constructing `ast::Select` is verbose and couples us
  to `sqlparser` 0.59's exact non-exhaustive field set, which drifts across
  minors — a known maintenance cost paid on every `sqlparser` bump (re-verify the
  field set, re-run the snapshot corpus, update this ADR). COPY remains a string
  template, so any future COPY-option change is hand-edited, not AST-checked.
- **Neutral:** the inner-SELECT-vs-COPY split mirrors the Phase 1 doc-comment
  contract; expression rendering is unchanged (still `render_expr`). Join /
  GroupBy / Aggregate codegen (the two-input + grouping ops) is deferred to plan
  04-05, which extends the same `ast.rs` surface.

## Alternatives considered

- **Full `format!` string templates (Phase 1 style) for everything.** Rejected:
  fragile identifier/literal quoting and `DISTINCT ON` / `EXCLUDE` / `UNION`
  spelling; harder to keep the byte-stable snapshot corpus green as ops grow.
- **Full AST including the COPY statement.** Rejected: `sqlparser` 0.59
  `Statement::Copy::to_string()` drops `(FORMAT PARQUET)` (Pitfall 1), so the
  emitted SQL would be wrong.
- **A third-party SQL builder crate.** Rejected: would add a dependency (and a
  WASM-cleanliness audit) for what is a thin, localised construction surface;
  `sqlparser` is already wired and proven WASM-clean.
