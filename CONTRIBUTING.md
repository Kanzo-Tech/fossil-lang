# Contributing to Fossil

Fossil is in **pre-v0.1 active development** — nothing is published and the
surface is still changing. This document covers what exists today and the
contributor rituals that make a solo project survivable.

## Development setup

Fossil targets Rust 1.90 (per `rust-toolchain.toml`). With `rustup`
installed, `cd`-ing into the repo automatically activates the right
toolchain plus the `wasm32-unknown-unknown` target.

Verify with:

```bash
rustc --version          # rustc 1.90.0
rustup target list --installed | grep wasm32
```

Optional one-time installs:

```bash
cargo install wasm-bindgen-cli --version 0.2.120 --locked
cargo install cargo-deny --locked
brew install binaryen   # macOS — for wasm-opt size optimization
```

## Build & test commands

```bash
cargo check --workspace                                      # native, all crates
cargo test --workspace                                       # native tests
cargo fmt --all -- --check                                   # format check
cargo clippy --workspace --all-targets -- -D warnings        # lint check
cargo deny check                                             # advisories + licenses + bans
cargo xtask wasm-check                                       # WASM gate (highest-leverage)
```

CI runs all of these on every PR. See `.github/workflows/ci.yml`.

The WASM gate takes no crate list. `crates/xtask` derives it from the resolved
dependency graph — the closure of the workspace's cdylib crates — and prints
what it checked. It exists because the two hand-written `-p …` lists that
preceded it, one in `ci.yml` and one in the `.cargo` alias, had already drifted
apart; the comment at the top of `crates/xtask/src/main.rs` records that. Do not
reintroduce a list, here or anywhere else.

The gate needs a wasm-capable `clang` for `fossil-df-wasm`'s `zstd-sys` (Apple
clang is not one; CI installs LLVM and sets `CC_wasm32_unknown_unknown`).
Without it, check the rest of the closure directly:
`cargo check --target wasm32-unknown-unknown -p fossil-wasm -p fossil-graph-wasm`.

## Development cycle

1. Read `CLAUDE.md` for the standing rules, and `git log --oneline` for where the
   work actually is.
2. Make your change.
3. Run `cargo fmt`, `cargo clippy`, the WASM gate, and any relevant `cargo test`.
4. Write the decision into the reference if your change is a non-obvious choice (see
   «Where a decision goes» below).
5. Commit atomically with a conventional-commit subject (see Commit policy below).
6. If you're stepping away for >1 week, update `RETURNING.md` (gitignored, local-only)
   describing where you are, what's broken, and the next 3 steps.

## Commit policy

- **Conventional commits**: `type(scope?): subject`. Types: `feat`, `fix`, `docs`,
  `refactor`, `test`, `chore`, `ci`, `build`, `style`, `perf`.
- **Imperative mood**: "add X", "fix Y" — never "added"/"adds"/"adding".
- **Subject ≤72 chars.** Body wraps at ~72 and explains WHY when non-obvious.
  Reference the page that states the rule, a measurement, or a pitfall
  (`Mitigates P-CRIT-2`) — never a record number.
- **Atomic**: one commit = one logical change. CI must pass on every commit, not just
  the tip of a branch.
- `Co-Authored-By: Claude <model> <noreply@anthropic.com>` trailer on Claude-authored
  commits, naming the model that actually wrote it — the log carries three so far, and
  `git log --format='%(trailers:key=Co-Authored-By)' | sort -u` is the list. Do not copy
  a model name out of this file; read the one you are.
- **Never** `--no-verify`, `--amend` to published commits, or use WIP / "fix stuff" subjects.

## Where a decision goes

Any decision between alternatives that took **more than 15 minutes to decide** gets
written down within 24 hours — and it gets written down **in the reference**, not in a
record beside it.

This used to be a directory of numbered records, and the directory is the reason the
rule now reads the way it does. Sixty-three of them accumulated, twelve saying
`proposed` while the thing was built, and the prose that cited them rotted around
them: **159 dead references** measured in versioned prose, and **eighteen of twenty**
citations of one record resolving to a *different* record. Two files three modules
apart asserted opposite things about the same rule, each citing an amendment, one of
them repealed. A second reference does not stay true; it stays *cited*.

So:

- **The decision itself** is the page that states the rule. `apps/docs/content/docs/design/`
  for the language, `apps/corpus/content/docs/` for the artifact, `grammar.bnf` for the
  syntax, `apps/docs/content/docs/book/typing.mdx` for the static semantics.
- **The alternative you rejected** goes to `design/discarded.mdx`, in its four fields —
  the idea, why it is attractive, the evidence against it, and **what would bring it
  back**. An entry that cannot state the last field does not go on the page. That field
  is what makes a decision reopenable rather than dogma, and it is the whole reason the
  page exists.
- **Anything you read to decide it** goes to `design/prior-art.mdx`, named. "Seven
  languages, no exception" is not a checkable claim until the seven are on the page.
- **A number you measured** goes on the page that makes the claim, beside the claim.

There is no `Status` field, because a page has no status: it says what is true, or it
carries a `today:` register that says what is true *yet*, with the file that would go
red. `apps/docs/content.test.ts` fails the build if a page claims either without
naming something checkable.

## RETURNING.md ritual

Solo + open timeline implies inevitable breaks. Without a returning ritual, by the
third break the codebase becomes opaque to its own author.

`RETURNING.md` is gitignored (`CLAUDE.local.md`-style) and updated when you anticipate
stepping away. Format:

```markdown
# RETURNING.md — last updated YYYY-MM-DD

## Where I left off
Working on <task / phase / plan>. Last commit: <SHA>.

## What's broken
- <issue 1 with reproducer>
- <issue 2>

## Next 3 steps
1. <concrete step>
2. <concrete step>
3. <concrete step>

## Tried-it / don't-redo
- <approach A — failed because Y>
- <approach B — moved on to C>
```

Read it on return BEFORE any code change. Mitigates P-SOLO-2.

## Pre-commit hooks

Optional and per-developer, not enforced via tooling. CI is the gate. If you want
local enforcement, put `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo xtask wasm-check` in your own
`.git/hooks/pre-commit` and `chmod +x` it. Note: that slows commits by ~30s
(clippy) + ~15s (WASM check); skip it for fast iteration loops.

## Releasing

One rmlext release publishes **three artifacts at the same version `vX.Y.Z`**, and keasy consumes
all three:

| Artifact | Registry | Workflow |
|---|---|---|
| `@fossil-lang/*` (npm packages) | npmjs.org (public) | `.github/workflows/release.yml` (changesets) |
| `ghcr.io/kanzo-tech/fossil:X.Y.Z` (`fossil` + `fossil-mcp`) | GHCR | `.github/workflows/fossil-image.yml` |
| `fossil-run-status` (wire-contract crate) | git tag `vX.Y.Z` | the tag itself (keasy uses a git-dep) |

The version's source of truth is the git tag `vX.Y.Z` that `changesets/action` creates when it
publishes. `fossil-image.yml` triggers on `push: tags: ['v*']`, so npm and the image land on the
same tag with no manual coordination.

**Nothing has been published yet, and three one-time operator actions gate the first release.**

1. **npm Trusted Publisher.** npmjs.com → scope `@fossil-lang` → Settings → Trusted Publishers →
   repository `Kanzo-Tech/fossil-lang`, workflow `release.yml`, environment blank. Without it the
   first `changeset publish --provenance` fails with a missing-OIDC error. **That is the gate
   working, not a bug** — do not debug it as one.
2. **The first stable version.** The linked `@fossil-lang/*` group sits at `0.3.0-alpha.0`. When
   merging the "Version Packages" PR, confirm it lands on `0.3.0` rather than jumping to `1.0.0`.
3. **GHCR visibility.** Public means keasy's `COPY --from` needs no auth, which is the simple path.
   Private means keasy's image build must `docker login ghcr.io` first, with an org-visible package
   or a PAT carrying `read:packages`. GitHub → org → Packages → `fossil` → visibility.

After that the per-release flow has no manual step beyond one merge: land changes with
`pnpm changeset`; `release.yml` opens the "Version Packages" PR; merging it publishes to npm and
creates the tag; the tag builds and pushes the image with provenance; keasy's Renovate opens one
grouped PR bumping the packages, the git-dep tag and the image together.

Three dry runs, none of which publish:

```bash
pnpm --filter @fossil-lang/wasm pack --dry-run   # confirm pkg/fossil_wasm_bg.wasm is listed
docker build -t fossil:local . && docker run --rm fossil:local --help
cargo build -p fossil-run-status --features utoipa   # the contract crate, isolated
```

## Issues, PRs, communication

Fossil is pre-public: no GitHub issue tracker, no PR workflow. Until it goes
public, communication is `angel.iglesias@kanzo.tech`.

## Code of conduct

Be kind. Be specific. Cite sources. The community we want to build is one where
"I read the page and disagree because X, Y" is the default discussion shape.
