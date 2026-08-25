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
cargo check --workspace --all-targets                        # native, all crates AND their tests
cargo test --workspace --no-fail-fast                        # native tests
cargo fmt --all -- --check                                   # format check
cargo clippy --workspace --all-targets -- -D warnings        # lint check
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" \
  cargo doc --workspace --no-deps                            # citations rustdoc can check
cargo deny check                                             # advisories + licenses + bans
cargo xtask wasm-check                                       # WASM gate (highest-leverage)
```

CI runs all of these on every PR. See `.github/workflows/ci.yml`.

**The two flags on the first two lines are load-bearing.** Without `--all-targets`,
`check` does not compile `tests/`, and a signature change that breaks four test
files reads as green — it did, on 2026-08-13. Without `--no-fail-fast`, `test`
stops at the first failing suite and reports the tests it happened to reach as if
they were the workspace. (CI omits `--no-fail-fast` deliberately: it wants the
first failure fast. You want the whole picture.)

Do not run the whole chain for every edit — it is six gates over a 1373-second CPU
build. Run the narrowest thing that could catch what you just did: one crate, one
test. If a build starts hurting, sweep first: `cargo sweep --time 1` reclaimed
205 GiB here, and the full test suite went back to ~6 minutes.

The WASM gate takes no crate list. `crates/xtask` derives it from the resolved
dependency graph — the closure of the workspace's cdylib crates — and prints
what it checked. It exists because the two hand-written `-p …` lists that
preceded it, one in `ci.yml` and one in the `.cargo` alias, had already drifted
apart; the comment at the top of `crates/xtask/src/main.rs` records that. Do not
reintroduce a list, here or anywhere else.

The gate needs a wasm-capable `clang` for `fossil-df-wasm`'s `zstd-sys` (Apple
clang is not one; CI installs LLVM and sets `CC_wasm32_unknown_unknown`). The
failure without one is `unknown target triple 'wasm32-unknown-unknown'` from
`cc-rs`. Homebrew LLVM is wasm-capable, and the full gate runs with it:

```bash
CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang \
AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar \
cargo xtask wasm-check
```

Without any LLVM at all, check the compiler closure directly — it is the smaller
claim: `cargo check --target wasm32-unknown-unknown -p fossil-wasm -p fossil-graph-wasm`.

## Generated files

Five files are generated and must not be hand-edited — two Rust, two TypeScript,
one MDX:

```
catalogue.bnf ──cargo xtask catalogue──▶ crates/fossil-base/src/providers/generated.rs
                                         crates/fossil-descriptors-output/src/generated.rs
                                         packages/introspect/src/catalogue.generated.ts
                                         packages/executor/src/catalogue.generated.ts

catalogue.bnf ─┐
               ├─cargo xtask catalogue──▶ apps/docs/content/generated/stdlib.mdx
fossil-hir ────┘   (crates/fossil-hir/src/stdlib.rs — the FunctionRegistry)
```

Each is a PROJECTION of the same rows, not a copy of the file: `fossil-base` gets
the rows whose behaviour the compiler can link, `descriptors-output` the ones
needing a shape-language parser, `introspect` the ones with a table function to
`DESCRIBE` through (`io.rdf` has none), `executor` every row that reads data
(`io.rdf` included — the host fetches its bytes like any other source). Which
projection a row lands in is derived from its clauses, never configured.

The fifth has a second source, because the catalogue has two halves and only the
`io.` one became a file. `FunctionRegistry` is already a table of values —
`RegistryEntry { name, recv, member, sig, lowering }` — so the emitter reads it
rather than the Rust that writes it down, and `apps/docs/content/docs/book/stdlib.mdx`
pulls each section in with `<include>` and keeps only the prose. It had written
the same table out by hand: 58 rows, of which **seven named nothing the checker
knows**, two of them (`io.sql`, `io.http`) occurring nowhere else in the
repository at all. `crates/xtask/src/reference.rs` has the measurement and the
argument for where each half comes from.

Add or change a row in `catalogue.bnf` **or in `fossil-hir`'s registry**, run
`cargo xtask catalogue`, commit what it wrote. `cargo xtask catalogue --check`
fails without writing, and there is no CI step for it on purpose:
`crates/xtask/tests/catalogue_generated.rs` is the same check as a test, so
`cargo test --workspace` already fails on a stale file and a second gate would be
one idea in two places. That file also holds the two guards the reference page
needs and `--check` cannot give it: that every registry row reaches the page, and
that the page has not gone back to writing a row of its own.

The `--check` proves each file matches its own emitter and nothing more. That the
Rust and TypeScript projections AGREE is a separate claim, and
`packages/introspect/tests/rust-parity.test.ts` is where it is checked — a `pnpm`
test, so `cargo test` will not tell you.

The file carries the argument for each row in its `(* … *)` commentary. A doc
comment in the generated Rust is derived and one line long; if you want to know
*why* `io.shex` refuses `.ttl`, that is in the `.bnf`.

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
  Reference the page that states the rule, a measurement, or the failure it
  avoids, spelled out ("keeps `tokio` out of the wasm32 closure") — never a
  record number, because there is no register left to resolve one against.
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
  for the language, `apps/docs/content/docs/format/` for the artifact, `grammar.bnf` for the
  syntax, `apps/docs/content/docs/book/typing.mdx` for the static semantics.
- **The alternative you rejected** goes to `design/discarded.mdx`, in its four fields —
  the idea, why it is attractive, the evidence against it, and **what would bring it
  back**. An entry that cannot state the last field does not go on the page. That field
  is what makes a decision reopenable rather than dogma, and it is the whole reason the
  page exists.
- **Anything you read to decide it** goes to `design/prior-art.mdx`, named. "Seven
  languages, no exception" is not a checkable claim until the seven are on the page.
- **A number you measured** goes on the page that makes the claim, beside the claim.

There is no `Status` field, because a page has no status. A page says what is true; one
describing something not yet built carries a `direction:` — one sentence, plus the page
here that argues for it — and that is the only mark. `apps/docs/content.test.ts` fails
the build if a direction has no summary, or names an argument that is not a page of the
site. There was a second register carrying a path to a file that would go red; it was
retired after eight of forty-nine such citations were measured pointing at a line that
said something else, with CI green. Evidence is transclusion now: a page reads the file
at build time, and an absent region stops the build.

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

Read it on return BEFORE any code change. It is the only defence against the
third break, after which the codebase is opaque to its own author.

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

Two dry runs, neither of which publishes:

```bash
pnpm --filter @fossil-lang/wasm pack --dry-run   # confirm pkg/fossil_wasm_bg.wasm is listed
docker build -t fossil:local . && docker run --rm fossil:local --help
```

## Issues, PRs, communication

Fossil is pre-public: no GitHub issue tracker, no PR workflow. Until it goes
public, communication is `angel.iglesias@kanzo.tech`.

## Code of conduct

Be kind. Be specific. Cite sources. The community we want to build is one where
"I read the page and disagree because X, Y" is the default discussion shape.
