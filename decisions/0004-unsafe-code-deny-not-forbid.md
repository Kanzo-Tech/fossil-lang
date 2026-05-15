# ADR 0004: Workspace lint `unsafe_code = "deny"`, not `"forbid"`

**Date:** 2026-05-15
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** Plan-checker review of Phase 1 plans (`.planning/phases/01-walking-skeleton-hello-fossil/01-02-PLAN.md` Task 3); Salsa 0.26 `Update` trait integration with rowan green-tree types

## Context

Phase 0 (commit 6bb78b4) set the workspace-level lint policy `[workspace.lints.rust] unsafe_code = "forbid"`. This was deliberate — Fossil v0.1 has no use case for raw pointer manipulation, FFI, or any other operation that would justify lifting the constraint. CLAUDE.md Hard Rules listed `unsafe_code = "forbid"` as a non-negotiable invariant.

Phase 1's `fossil-syntax` crate hits a wall: Salsa 0.26's `Update` trait must be implementable on the rowan-CST types stored inside `#[salsa::input]` and `#[salsa::tracked]` structs. The auto-derive `#[derive(salsa::Update)]` works for many types but not for `rowan::SyntaxNode` because rowan's tree representation uses interior `Arc` and shared green nodes — Salsa's blind `Update` derivation cannot reason about reference equality there. The canonical workaround across the Salsa-using compiler ecosystem (rust-analyzer, ty/red-knot, ruff_db) is to provide an explicit `unsafe impl salsa::Update for SyntaxNodeWrapper { ... }` at the integration boundary, with a justification comment.

`forbid` cannot be overridden by `#[allow(unsafe_code)]` on an inner item. `deny` can. The functional difference is: `forbid` says "no unsafe anywhere, full stop"; `deny` says "no unsafe by default, but with an explicit per-item override accompanied by justification, you may have a single named exception." For Phase 1 specifically, the only known unsafe site is the rowan/Salsa integration in `fossil-syntax`, and its semantics are well-understood. The choice is not "permit unsafe" vs "ban unsafe" — it is "ban with no escape" vs "ban with named exceptions tracked by reviewers."

The forces in tension: maximally restrictive lint policy versus the practical need for one carefully-bounded `unsafe impl` at a third-party-trait integration boundary. The reference codebases all chose `deny` + per-site `#[allow(unsafe_code)]` with comment justification.

## Decision

We will use `[workspace.lints.rust] unsafe_code = "deny"` (not `"forbid"`).

Per-item `#[allow(unsafe_code)]` is permitted ONLY at integration boundaries with third-party traits (Salsa `Update`, FFI bindings, etc.) and MUST carry a single-line justification comment naming what the unsafe is for and why no safe alternative exists. PRs that add `#[allow(unsafe_code)]` without justification, or that use `unsafe` for a reason other than third-party-trait integration, are rejected at review.

The workspace `Cargo.toml` is updated. CLAUDE.md is updated to match. The deny.toml `bans` table does not need changes (it bans crates by name, not by content).

## Consequences

**Positive:**
- Fossil can now adopt the canonical Salsa+rowan integration pattern from rust-analyzer / ty without forking or reimplementing rowan.
- Per-site justifications create an auditable record of every unsafe impl. Grep `git log -S 'allow(unsafe_code)'` shows them all.
- Aligns with reference codebases (rust-analyzer, ty, ruff_db all use `deny` with per-site exceptions, not `forbid`).
- The "no escape hatch" property of `forbid` was illusory — the alternative was to vendor or fork rowan, which is far worse.

**Negative:**
- Reviewers must remain vigilant: `#[allow(unsafe_code)]` is no longer a hard compiler error, so a missed PR review can land unjustified unsafe. Mitigation: ban unjustified additions explicitly in CONTRIBUTING.md; future tooling (e.g., a clippy lint or custom CI check) can grep PRs for new `#[allow(unsafe_code)]` and require a comment.
- Slight deviation from the "maximally cautious" stance some Rust-shop conventions prefer.

**Neutral:**
- Phase 1 will land at most 1-2 `#[allow(unsafe_code)]` sites (in `fossil-syntax` for rowan/Salsa). Future phases may add more at FFI boundaries (e.g., DuckDB callbacks in `fossil-runtime`), each requiring its own justification.
- The change supersedes CLAUDE.md's prior "unsafe_code = forbid" Hard Rule entry, which is updated in the same commit.
