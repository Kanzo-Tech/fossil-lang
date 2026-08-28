# apps/corpus — rules local to here

The repository rules are in `../../CLAUDE.md`. These are true only here.

## What this is, and what it stopped being

The **executable** half of the corpus contract. `guards/` checks that a corpus on disk satisfies the
conventions; `conformance/` checks that three independent readers address it the same way, and that
the corpus they address reproduces from a recorded recipe.

The prose half was a second Next.js site living beside this directory. It is gone: the pages are
`apps/docs/content/docs/format/`, and what it cost was nine runtime dependencies identical version
for version to the ones next door, a second bundler, a second search index and a second build, for
twelve pages. Its two server components moved with it —
`apps/docs/components/{guard-index,vector-table}.tsx` — and they still read the files here.

**That reach matters.** `guard-index.tsx` imports `guards/guards.mjs` and `vector-table.tsx` reads
`guards/vectors.json`, both at build time. Renaming or moving either one breaks the documentation
build, which is the intended coupling: a page cannot drift from a table it does not contain.

## Three readers, and the third one is not free

`conformance/expected.json` is a table of addresses that none of its executors wrote. Three read it:
`conformance/reader.mjs` (plain Node, from the conventions), `packages/corpus/src/address.ts` (the
published module, run from `packages/corpus/tests/conformance.test.ts`), and `fossil_graph::address`
— natively in `crates/fossil-graph/tests/conformance.rs`, and as wasm32 through
`conformance/wasm-reader.mjs`.

It was two, and **both were JavaScript**: a mistake they shared — a shift taken as signed, a count
that went through a `Number` — was invisible to a diff of the two, which is the shape of blindness
`GraphAr`'s fourth implementation landed through.

The wasm leg is the one thing here that needs more than `node` and a `duckdb` binary, and it is
therefore **refused explicitly rather than skipped**. `verify.mjs --without-wasm` is the opt-out and
`corpus.yml` is where it is passed; without it a missing `packages/corpus/pkg/` is a failure. Do not
change that default: a leg that vanishes with its dependency is how this stops being evidence
without anybody noticing.

## The checked-in corpus has a recipe, and the recipe is executed

`conformance/corpus/` is generated, and the parameters that generated it are `RECIPE` in
`conformance/provenance.mjs`. Regenerating with `guards/fixture.mjs`'s **defaults** gives 600 edges
against the 596 the manifest declares — which reads exactly like a stale fixture and is not one.
`clusters` decides how many chords the ring carries, the default is 256, and at 16 the generator
reproduces this corpus byte for byte. **Do not regenerate it with the defaults to "fix" the
mismatch.** That was diagnosed once from nothing but the numbers; the file exists so it is not
diagnosed again.

## `guards/` has no dependencies, and that is a constraint not an accident

Plain ESM, `node` plus the `duckdb` binary, no build. The directory is meant to be **copied** by a
third party who has neither this repository nor Rust nor pnpm — that is why it keeps its own README
when every other README in `crates/` was deleted. Adding an npm dependency changes what the contract
costs to check, which changes who can check it.

The rule is on `guards/`. `conformance/wasm-reader.mjs` reaches out of this app into
`packages/corpus/pkg/`, and it is the only file here that does; nothing under `guards/` may.

`pnpm test` writes a conforming corpus in both containers, requires every guard to pass, and then
requires every guard to **fire** against a corpus broken in exactly one way. A guard nobody has seen
fail is a sentence.

## Every guard declares what it cannot prove

`proves` and `cannotProve` are both required, the self-test asserts both are non-empty, and the page
renders them with equal weight. This is the price of documenting a format instead of shipping a type
for it, and it was accepted knowingly. Do not ship a guard without the second field, and do not
soften one: a guard that overstates its reach is worse than a missing one.

## No guard over the prose

The obvious one — assert every `file:line` a page cites is on disk — existed next door and has now
been deleted twice over. It proves a cited line *exists*, never that it *says* what the page claims:
159 dead references accumulated under the first version with CI green, and the second version was
passing over eight citations that had drifted to a different line, one of them blank. Do not rebuild
it here or there.

What replaces it is rendering rather than transcribing, which is what the two components above do,
and transclusion — `<Program src= region= />` on the documentation side reads the file at build time
and an absent region stops the build.

## Never published

`private: true`, and the root's recursive scripts are scoped to `./packages/*` by path. The
`pnpm-workspace.yaml` entry exists for resolution, not for npm.
