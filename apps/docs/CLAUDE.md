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
  - `direction:` — `summary` (one sentence, where this is going) + `decidedBy` (a path under
    `decisions/`).
  - `today:` — `summary` (one sentence, what is true right now) + **exactly one** of
    `backedBy: <path>` or `unmeasured: true`.
- Paths are relative to the **repository root**, not to this app, because that is how the ADRs and
  `git log` write them.
- `source.config.ts` checks the *shape*; `content.test.ts` checks *presence* and that every cited
  path is still on disk. Neither subsumes the other — a zod schema does not know which directory a
  file came from, and the index and the decision list carry no registers by design.
- **`build` runs `test` first, and that is load-bearing.** Renaming a test out from under a page it
  backs has to break the docs build rather than rot in prose. Verified by breaking it five ways;
  `next build` on its own does not catch any of them.

What the guard cannot prove, and the prose must therefore say: that a `backedBy` path actually
*tests* the claim. It checks the file is there, not that it asserts anything.
`characteristics/bounded-write.mdx` points at an implementation file because the spill test ADR-0043
stage 4 specifies has not been written, and the page says so in a callout. Do not quietly downgrade
a page to a weaker citation — say it on the page.

## No design system yet

**TODO(ADR):** `@kanzo-tech/ui` is the intended design system and is **not on npm** — `npm view
@kanzo-tech/ui` returns 404 as of 2026-08-05. Consuming it means vendoring tarballs across two
repositories, which is a cross-repo dependency that needs its own decision record before any of it
is wired. (There are uncommitted edits in `packages/{editor,viewer}` doing exactly that via
`file:../../../kanzo-ui/packages/ui`, and they break `pnpm install` in this workspace today, which
is the argument for the ADR rather than against it.)

Until that record exists this app dresses in fumadocs' own preset plus the handful of rules at the
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
