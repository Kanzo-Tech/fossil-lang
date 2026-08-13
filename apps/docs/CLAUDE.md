# apps/docs — rules local to this app

The repository rules are in `../../CLAUDE.md`. These are true only here.

## The editorial stance, which is the whole point

This site documents **where fossil is going**, written before the code gets there. The destination
is the primary content, not a footnote; what is already true is marked as such, with the thing that
proves it.

- **Never mix the two registers in one sentence.** A target restated as a fact is the failure this
  site exists to prevent, and it is invisible in a diff.
- Every page under `content/docs/characteristics/` and `content/docs/protocols/` carries both
  frontmatter blocks:
  - `direction:` — `summary` (one sentence, where this is going) + `arguedIn` (a `/docs/…` route on
    this site, and it may not be the page itself).
  - `today:` — `summary` (one sentence, what is true right now) + **exactly one** of
    `backedBy: <path>` or `unmeasured: true`.
- `backedBy` paths are relative to the **repository root**, not to this app, because that is how
  `git log` writes them. `arguedIn` is not a path at all — it is a route, because the argument is a
  page here.
- `source.config.ts` checks the *shape*; `content.test.ts` checks *presence*, that every cited path
  is still on disk, and that every `arguedIn` resolves to a different page of this site. Neither
  subsumes the other — a zod schema does not know which directory a file came from, and the index
  carries no registers by design.

- **`build` runs `test` first, and that is load-bearing.** Renaming a test out from under a page it
  backs has to break the docs build rather than rot in prose. Verified by breaking it five ways;
  `next build` on its own does not catch any of them.

What the guard cannot prove, and the prose must therefore say: that a `backedBy` path actually
*tests* the claim. It checks the file is there, not that it asserts anything. Do not quietly
downgrade a page to a weaker citation — say it on the page, the way
`characteristics/bounded-write.mdx` does in its callout.

## One reference, and this site is most of it

There is no `decisions/` directory and no design document in the repository root. Both existed, both
were a second reference, and both produced the failure this site's guards exist to catch: **159 dead
citations** measured in versioned prose, and **eighteen of twenty** citations of one record
resolving to a *different* record. The argument for a direction lives on a page here — `design/`,
with `design/discarded` for the rejected alternatives and `design/prior-art` for the sources — and
`grammar.bnf` plus `book/typing` are the two normative documents outside it.

**Do not reintroduce a record.** If a decision needs somewhere to live, it is a page; if a page
cannot say what would reverse it, `design/discarded`'s own admission rule says it does not go on.

## No design system yet

`@kanzo-tech/ui` is the intended design system and is **not on npm** — `npm view @kanzo-tech/ui`
returns 404 as of 2026-08-05. Consuming it means vendoring tarballs across two repositories, which
is a cross-repo dependency that needs its own page and its own argument before any of it is wired.
(There are uncommitted edits in `packages/{editor,viewer}` doing exactly that via
`file:../../../kanzo-ui/packages/ui`, and they break `pnpm install` in this workspace today, which
is the argument for writing that page rather than against it.)

Until it is written this app dresses in fumadocs' own preset plus the handful of rules at the
bottom of `app/global.css`. All of it is meant to be deleted, not extended.

## No `--webpack`

The kanzo-ui docs app pins that flag because vgplot trips a temporal-dead-zone error under
Turbopack and its charts do not mount without it. **That does not apply here** — no charts, no
vgplot, no WebGL. Verified: `next build` is green on the default bundler. Do not copy the flag.

## Never published

`private: true`, and every recursive step in CI is scoped to `./packages/*` by path. The
`pnpm-workspace.yaml` entry exists for resolution, not for npm. `.github/workflows/docs.yml` is this
app's only gate and it is path-filtered; `pnpm-ci.yml` deliberately does not build it, because that
job already sits at a 60-minute ceiling for a cold cargo build.
