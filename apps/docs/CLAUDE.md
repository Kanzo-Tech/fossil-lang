# apps/docs — rules local to this app

The repository rules are in `../../CLAUDE.md`. These are true only here.

## One site, and this is all of it

There is no `decisions/` directory, no design document in the repository root, and no second
documentation app. All four existed. The first two produced the failure this site's guards were
built to catch — **159 dead citations** in versioned prose, and eighteen of twenty citations of one
record resolving to a *different* record. The third was `SURFACE-PLAN.md`, 1,443 lines, which
diagnosed itself on its own line 5 and had been cited 89 times from the code by the time it went;
`crates/xtask/tests/no_second_reference.rs` now fails if prose reappears at the root. The fourth was
`apps/corpus`, a whole second Next.js app — nine runtime dependencies identical version for version
to these — serving twelve pages that are now `content/docs/format/`.

**Do not reintroduce any of them.** If a decision needs somewhere to live, it is a page; if a page
cannot say what would reverse it, `design/discarded`'s own admission rule says it does not go on.

## Two kinds of page, and they are marked differently

**Most pages describe what is there.** No frontmatter beyond title and description, and the prose is
in the present tense because the thing exists.

**A page describing something not yet built declares a `direction:`** — one sentence saying where
fossil is going, plus an optional `arguedIn` route to the page here that argues for it. Declaring it
is the whole of the opt-in; there is no governed directory. `source.config.ts` checks the shape,
`content.test.ts` checks that a declared direction has a summary and that a named argument resolves
to a *different* page of this site.

A direction must be a property that can one day be declared closed. A number is not a property
("twenty-five crates towards five to eight" measured Gleam, not fossil). A perpetual negative is not
a direction — "never grows a planner beside the one underneath it" can only ever be broken, so it
became a grep in CI. And a direction nobody is waiting for is not a direction: RDF 1.2 had zero
tests, zero consumers and a retired grammar rule, so it is an entry in `design/discarded` instead.

## `content/docs/format/` has one tense

The corpus format is a specification, and a specification does not have a roadmap in its prose.
**Never write "today", "currently", "not yet", "will be", "for now" on those pages.** A convention
holds of a corpus or it does not, and what says which is `apps/corpus/guards/check.mjs`, not an
adverb. Where something is genuinely undecided, write it as a **design question** naming what would
settle it — which is not the same as a gap.

Numbers there come from measuring, not from memory. There are **six** verbs, not fourteen or
seventeen; `fossil-http` has never existed. When a number and a document disagree, read the code —
this repository has now cashed that lesson roughly a dozen times in one sitting.

## Evidence is transclusion, not citation

There used to be a `today:` register carrying a `backedBy:` path to a file that would go red if the
sentence stopped holding. It is gone, and the measurement is the argument: of forty-nine `file:line`
citations, **eight had drifted to a line saying something else** — one to a blank line, one
fifty-three lines adrift — with CI green throughout, because the guard could prove the file existed
and nothing more. And `unmeasured: true`, the honest alternative the schema offered, was used by
**zero of nine** pages: every one preferred a weak citation to admitting there was no evidence.

So prefer `<Program src= region= />`, which reads the file at build time — an absent region stops
the build, and the code on the page *is* the code on disk. Same for `<GuardIndex />` and
`<VectorTable of= />`, which render `apps/corpus/guards/{guards.mjs,vectors.json}` rather than
transcribing them.

**A line number is not a citation.** This paragraph used to end "a bare `file:line` is allowed and is
your responsibility to verify by opening it", and that policy was measured: of **35 citations, 13
said something other than what the page claimed** — 37%, one of them a blank line, one naming a test
the same sentence had already named correctly in prose. Nobody re-reads a paragraph to check a
number, and the guard could only prove the line was in range. So the spelling is the one
`grammar.bnf` already uses:

    `crates/fossil-hir/src/lower.rs, HirExpr`

The anchor is an item the file DEFINES — `fn`, `struct`, `enum`, `trait`, `type`, `mod`, `const`,
`static`, `macro_rules!` — and `content.test.ts` checks it. A `use` that merely names the item does
not count, because four of the thirteen cited the import for a claim about the function underneath.
Where the claim is «this is pinned in that file» and no single item carries it, write the path with
no line and no anchor; nothing checks that, and `design/discarded` records why and what would change
it. `path:12` in any form fails the build.

## What the guards hold, and what they cannot

`build` runs `test` first, and that is load-bearing. `content.test.ts` fails the build on: a
direction without a summary, an `arguedIn` that does not resolve or that names its own page, a
`/docs/…` link to a page that is not there, a `#fragment` naming no heading in the target, a source
citation naming an item its file does not define, a citation written as `path:12` at all, and a
`grammar.bnf` citation that names no production. It also carries the one architectural claim a
manifest can settle.

Each of those has a vacuity check beside it, because a glob that stops matching is how a guard
stops guarding without anyone noticing.

**A vacuity check is not a floor, and there is no floor.** These suites are `it.each` over what the
tree contains, so deleting the last citation on a page deletes its tests and the suite shrinks
without going red. That is deliberate and it has no honest fix: deleting a citation because the
claim became a `<Program>` is what this page asks for, and it is the same diff as deleting one
because the sentence quietly stopped carrying evidence. Nor does the headline number help — one
conversion took the citation suite from 36 to 35 while the total stayed at 601, because the link
suite grew by one at the same time. `design/discarded` carries the rejected floor.

**What none of them prove: that the sentence is true.** No test of this shape will. They make a
claim locatable and falsifiable; the rest is reading.

## No design system yet

`@kanzo-tech/ui` is the intended one and is not on npm, so consuming it means vendoring tarballs
across two repositories — a cross-repo dependency that needs its own argued page first. Until then
this app dresses in fumadocs' own preset plus the handful of rules at the bottom of
`app/global.css`, and all of it is meant to be deleted rather than extended.

## No `--webpack`, and never published

The kanzo-ui docs app pins that flag because vgplot trips a temporal-dead-zone error under
Turbopack. **That does not apply here** — no charts, no vgplot, no WebGL, and `next build` is green
on the default bundler. Do not copy it.

`private: true`, and every recursive step in CI is scoped to `./packages/*` by path.
`.github/workflows/docs.yml` is this app's only gate and it is path-filtered; `pnpm-ci.yml`
deliberately does not build it, because that job already sits at a 60-minute ceiling for a cold
cargo build.
