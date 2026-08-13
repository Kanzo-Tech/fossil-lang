# apps/corpus — rules local to this app

The repository rules are in `../../CLAUDE.md`. These are true only here.

## What this app is

The corpus half of fossil, documented as what it is: a **format**. The language is the other half
and has its own site; nothing here depends on it, and a page that needs the reader to know the
language has drifted.

Two deliverables, and the second is the load-bearing one:

- `content/docs/` — the conventions, each with the measurement that produced it.
- `guards/` — executable checks that a corpus on disk satisfies them.

## One tense

A specification has one tense. **Never write "today", "currently", "not yet", "will be", "for now".**
A convention either holds of a corpus or it does not, and the thing that says which is
`guards/check.mjs`, not an adverb.

This differs from the language site on purpose. That site documents a destination, so it carries two
registers per page and a schema keeping them apart. Do not import that machinery here — a format
with a roadmap in its prose is a format nobody can implement against.

Where something is genuinely undecided — the tile payload format is the live one — write it as a
**design question**, with what would settle it. That is not the same as a gap.

## Numbers come from the records, not from memory

Every figure on this site is measured and traceable to a decision record. Two that are commonly
wrong and were wrong in the older prose: there are **six** verbs, not fourteen or seventeen; and
`fossil-http` has never existed. When a number and a document disagree, read the code.

## `guards/` has no dependencies, and that is a constraint not an accident

Plain ESM, `node` plus the `duckdb` binary, no `package.json`, no build. The directory is meant to be
**copied** by a third party who has neither this repository nor Rust nor pnpm. Adding an npm
dependency to it changes what the contract costs to check, which changes who can check it.

`pnpm test` is `node guards/self-test.mjs`, and it runs ahead of `next build`. It writes a conforming
corpus in both containers, requires every guard to pass, and then requires every guard to **fire**
against a corpus broken in exactly one way. A guard nobody has seen fail is a sentence.

## Every guard declares what it cannot prove

`proves` and `cannotProve` are both required fields, the self-test asserts both are non-empty, and
`components/guard-index.tsx` renders them with equal weight. This is the price of documenting a
format instead of shipping a type for it, and it was accepted knowingly. Do not quietly ship a guard
without the second field, and do not soften one — a guard that overstates its reach is worse than a
missing one.

## No guard over the prose

The obvious one — assert every `file:line` a page cites is on disk — existed next door and was
audited out. It checks that the cited line *exists*, never that it *says* what the page claims, and
**159 dead references** accumulated under it with CI green, one page pointing at a blank line. Do not
rebuild it here.

What replaces it is rendering rather than transcribing: `components/guard-index.tsx` reads
`guards/guards.mjs` and `components/vector-table.tsx` reads `guards/vectors.json`, both at build
time. A page cannot drift from a table it does not contain.

## Never published

`private: true`, and the root's recursive scripts are scoped to `./packages/*` by path. The
`pnpm-workspace.yaml` entry exists for resolution, not for npm.

## No design system yet

Same position as the language site: `@kanzo-tech/ui` is the intended one and is not on npm, so
consuming it means vendoring tarballs across two repositories — a cross-repo dependency that needs
its own decision record first. Until then this app dresses in fumadocs' own preset plus the two
rules at the bottom of `app/global.css`, and all of it is meant to be deleted rather than extended.
