# Contributing to Fossil

Fossil is in **pre-v0.1 active development**. This document covers what
exists today (Phase 0 complete; Phase 1 walking-skeleton next) and the
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

# WASM gate (the highest-leverage check; 6 compiler-core crates)
cargo check --target wasm32-unknown-unknown \
    -p fossil-base -p fossil-syntax -p fossil-hir \
    -p fossil-mir -p fossil-codegen -p fossil-wasm
```

CI runs all six on every PR. See `.github/workflows/ci.yml`.

## Development cycle

1. Read `.planning/STATE.md` for current focus.
2. Make your change.
3. Run `cargo fmt`, `cargo clippy`, the WASM gate, and any relevant `cargo test`.
4. Write an ADR if your change is a non-obvious choice (see ADR ritual below).
5. Commit atomically with a conventional-commit subject (see Commit policy below).
6. If you're stepping away for >1 week, update `RETURNING.md` (gitignored, local-only)
   describing where you are, what's broken, and the next 3 steps.

## Commit policy

- **Conventional commits**: `type(scope?): subject`. Types: `feat`, `fix`, `docs`,
  `refactor`, `test`, `chore`, `ci`, `build`, `style`, `perf`.
- **Imperative mood**: "add X", "fix Y" — never "added"/"adds"/"adding".
- **Subject ≤72 chars.** Body wraps at ~72 and explains WHY when non-obvious.
  Reference ADRs (`Per ADR-001`), research findings, or pitfalls (`Mitigates P-CRIT-2`).
- **Atomic**: one commit = one logical change. CI must pass on every commit, not just
  the tip of a branch.
- `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>` trailer on
  Claude-authored commits.
- **Never** `--no-verify`, `--amend` to published commits, or use WIP / "fix stuff" subjects.

## ADR ritual

Any decision between alternatives that took **more than 15 minutes to decide** gets
an ADR within 24 hours of the decision.

ADRs are how a solo project survives the bus-factor-1 problem. They are also how
Future-Angel reads Past-Angel's reasoning without ambient context.

To write one:

```bash
cp decisions/template.md decisions/NNNN-verb-noun-phrase.md
# fill in Title / Date / Status / Decider / Cite / Context / Decision / Consequences
# update decisions/README.md index table
git add decisions/ && git commit -m "docs(adr): record decision NNNN: <one-line>"
```

Examples to follow: `decisions/0001-use-lsp-server-not-tower-lsp.md`,
`decisions/0002-fifteen-crate-workspace-layout.md`,
`decisions/0003-thin-db-trait-with-system-abstraction.md`.

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

Optional and per-developer, not enforced via tooling. CI is the gate. If you
want local enforcement, see the `.git/hooks/pre-commit` snippet in
[`.planning/phases/00-workspace-genesis-rudof-spike/00-RESEARCH.md`](.planning/phases/00-workspace-genesis-rudof-spike/00-RESEARCH.md)
— copy it into your local `.git/hooks/pre-commit` and `chmod +x`. Note: this
slows commits by ~30s (clippy) + ~15s (WASM check); skip for fast iteration loops.

## Issues, PRs, communication

Fossil is pre-public. Until Phase 9 (public release), open communication via
`angel.iglesias@kanzo.tech` or the `decisions/` directory (ADR-shaped issues).

After Phase 9, the project goes public on GitHub with standard issue tracker /
PR workflow + W3C kg-construct mailing list announcement.

## Code of conduct

Be kind. Be specific. Cite sources. The community we want to build is one where
"I read your ADR-0042 and disagree because X, Y" is the default discussion shape.
