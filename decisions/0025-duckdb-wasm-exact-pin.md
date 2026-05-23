# ADR 0025: Pin `@duckdb/duckdb-wasm` to exact `1.32.0` in the production playground; the SC#1 parity harness keeps the `latest` dev floater

**Date:** 2026-05-23
**Status:** accepted
**Decider:** Angel Iglesias
**Cite:** ADR-0014 (SC#1 two-tier parity); 07-RESEARCH §"Open question 6"; 07-04 deviation (1.33.0 → 1.32.0 bump); `.planning/phases/06-cli-complete-lsp/deferred-items.md` (exact-pin repin flagged for Phase 7)

## Context

Two near-duplicate concerns landed on this decision at once.

First, **ADR-0014's pin policy is loose by design for the parity harness**: the harness intentionally tracks the npm `latest` tag for `@duckdb/duckdb-wasm` because the SC#1 baseline-versus-WASM diff *needs* to follow the upstream train (the whole point of the manual phase-close re-run is to catch a divergence introduced by a DuckDB-WASM bump). That float is appropriate for a harness that re-runs on every phase close.

Second, **the production playground needs the opposite policy**. The playground ships to end users via a static build; a floating `latest` would mean every CI build of the playground could pull a different DuckDB-WASM binary, which contradicts the whole "pin everything" approach (Pitfall 7 of the Phase-7 plan corpus) we apply to monaco-editor / monaco-languageclient / vscode-languageclient / wasm-bindgen-cli.

Plan 07-04 already discovered that **`1.33.0` does not exist on npm** — only `1.33.1-devN.0` pre-releases — and bumped the playground pin to the last fully-stable tag, `1.32.0` (recorded as a Rule 3 deviation in the 07-04 summary). That bump implicitly settled the version question; this ADR formalises it and reconciles with ADR-0014.

We considered three policies for the playground pin:

  a. `"@duckdb/duckdb-wasm": "1.32.0"` — exact (current state on disk).
  b. `"@duckdb/duckdb-wasm": "^1.32.0"` — caret (minor + patch float).
  c. `"@duckdb/duckdb-wasm": "~1.32.0"` — tilde (patch float).
  d. `"@duckdb/duckdb-wasm": "latest"` — match ADR-0014's harness policy.

Caret was rejected because the DuckDB-WASM minor-version line has historically broken loader contracts (`selectBundle` signature changes, jsDelivr bundle layout changes). Tilde was rejected at higher resolution for the same reason — a new patch can drag in a `dev` worker-loader rewrite. `"latest"` was rejected because the playground build is end-user-facing; we want a reviewable diff when the binary changes, not an invisible CDN drift.

## Decision

We will pin `playground/package.json` to **exact `1.32.0`** (no caret, no tilde) for `@duckdb/duckdb-wasm`. Future bumps go through an ADR amendment on this file (or a superseding ADR for a major-version move) so the bump's risk surface is reviewable; the in-tree parity harness (`tests/wasm_parity/`) keeps the `latest` floater per ADR-0014 because its job is to track the upstream train.

When a `1.33.x` *stable* lands on npm, the playground pin moves to that exact tag in a one-line `package.json` edit + an amendment footer on this ADR; the parity harness re-runs to confirm the digests are unchanged.

## Consequences

- **Playground builds become deterministic.** A `git pull` + `npm ci` produces the same DuckDB-WASM binary on every machine; CI cannot drift silently.
- **Future DuckDB-WASM bumps are reviewable.** The pin sits in `package.json` as a single number; bumping it is a one-line PR + a one-paragraph amendment on this ADR (the same ritual as monaco-editor, monaco-languageclient, etc.).
- **Two-version reality acknowledged.** The playground sees `1.32.0`; the parity harness sees `latest` (a `1.33.x` dev pre-release until a final lands). The asymmetry is documented in both places + cross-referenced via ADR-0014's footer.
- **Risk of stagnation.** If `1.32.0` accumulates a known browser-loader bug and we forget to bump, the playground inherits it. Mitigated by: (a) the parity harness's `latest` float surfaces upstream regressions at every phase close; (b) the bump procedure is documented (one-line + footer amendment).
- **No runtime divergence between native and browser within the playground itself.** The playground SQL is the same SQL the SC#1 parity harness verifies — exact-pinning the playground does not weaken SC#1's parity claim, because the harness owns the cross-engine diff (ADR-0014).

## Alternatives considered

- **Caret `^1.32.0` (minor+patch float).** Rejected: a DuckDB-WASM minor bump can rewrite the worker-loader contract; we want a reviewable diff when that happens.
- **Tilde `~1.32.0` (patch float).** Rejected: pre-1.x patch releases in this ecosystem regularly ship feature changes; the patch float is not safe.
- **`"latest"` (match the harness policy).** Rejected: the production playground is end-user-facing; CDN-style drift is invisible and removes the reviewable-diff guarantee.
- **Move the parity harness off `latest` too.** Rejected: the harness exists precisely to track upstream; an exact pin there would defeat its purpose. ADR-0014 already explains the harness's float policy.
