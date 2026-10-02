# packages/corpus — rules local to here

The repository rules are in `../../CLAUDE.md`. These are true only here.

## What this is

The published reader, `src/`, and beside it the **executable** half of the corpus contract it
fulfils: `guards/` checks that a corpus on disk satisfies the conventions, `conformance/` holds a
checked-in corpus with the recipe that reproduces it, and `integration/` writes corpora — with the
guards' own fixture, and with the real writer (`@fossil-lang/executor`, in `round-trip.test.ts`,
served over HTTP) — and reads them back through `src/`. None of the three is in `files`, so npm
never sees them. The prose half is `docs/content/docs/format/`.

`src/` is TypeScript and nothing else: `open` reads `fossil.json` through `JSON.parse`, creates a
view per table and two relations of the manifest — `fossil_tables`, `fossil_columns` — and answers
the function that detaches them. **Every read after that is the host's SQL** (Mosaic's, in keasy).
Do not put a query API back here — `scan` and its `Filter` were a second, poorer copy of SQL, and the
one consumer translated Mosaic's predicates into it — and do not export the manifest's types: a
caller reads the manifest as the two relations. The writer's `fossil_sinks::manifest` types are the
one definition; `tests/manifest.test.ts` validates every fixture against the schema generated from
them (`crates/fossil-sinks/fossil.schema.json`).

`guards/guards.mjs` is imported by `docs/components/guard-index.tsx` at build time, so renaming or
moving it breaks the documentation build — the intended coupling: a page cannot drift from a table
it does not contain.

## `guards/` has no dependencies, and that is a constraint not an accident

Plain ESM, `node` plus the `duckdb` binary, no build. The directory is meant to be **copied** by a
third party who has neither this repository nor Rust nor pnpm. Adding an npm dependency changes what
the contract costs to check, which changes who can check it. Nothing under `guards/` may import
`src/`; `integration/` may import both.

`pnpm test:contract` writes a conforming corpus, requires every guard to pass on it, and then
requires every guard to **fire** against a copy broken in exactly one way. A guard nobody has seen
fail is a sentence.

## Every guard declares what it cannot prove

`proves` and `cannotProve` are both required, the self-test asserts both are non-empty, and the
format index renders them with equal weight. Do not ship a guard without the second field, and do
not soften one: a guard that overstates its reach is worse than a missing one.

## The checked-in corpus has a recipe, and the recipe is executed

`conformance/corpus/` is generated: `RECIPE` in `conformance/provenance.mjs` is the command, and
`provenance.mjs` regenerates into a temporary directory and requires the same manifest text and the
same rows in the same order. Regenerate it only with that command; never edit it by hand.

## `test` is the reader's, and the contract has its own names

`pnpm test` is `vitest run` over `tests/` and nothing else, because `release.yml` runs
`pnpm --filter "./packages/*" test` before it publishes, on a runner with no `duckdb` binary, and a
suite that spawned it in that step is what took `v0.3.0-alpha.4` down. `tests/` reads the checked-in
corpus through DuckDB-WASM, which is a dev dependency and not a binary. The contract runs under
`test:contract` (guards and provenance) and `test:integration` (`integration/`, under
`vitest.integration.config.ts`). **Do not fold either into `test`.**
