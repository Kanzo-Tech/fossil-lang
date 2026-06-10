---
"@fossil-lang/codemirror-fossil": minor
"@fossil-lang/wasm": minor
"@fossil-lang/types": minor
"@fossil-lang/resolvers": minor
"@fossil-lang/examples": minor
"@fossil-lang/editor": minor
"@fossil-lang/viewer": minor
---

v0.2.0 is the structural release. The `@fossil-lang/*` package family is
complete — 6 pre-existing packages graduate from v0.1.x plus 2 new ones
(`@fossil-lang/editor`, `@fossil-lang/viewer`) ship for the first time. The
Keasy migration (Phase 16) validated the multi-host pattern against a real
closed-source SaaS sibling product — 707 LOC of duplicate component code
deleted from Keasy's web tree, now sourcing from the shared library.

### Package summary

  through Phases 9–14 (Reset semantics, dark/light/custom themes, WCAG 2.1
  AA accessibility audit, deferred-initialization performance fix).
- **`@fossil-lang/codemirror-fossil`** — language extension stabilized; same
  WASM-tokenizer single source of truth (ADR-0030). Keasy uses it both
  directly (via `@fossil-lang/editor`) and through the LSP transport layer.
- **`@fossil-lang/wasm`** — bumped to track Rust compiler crate evolution;
  byte-identical JS wrapper API.
- **`@fossil-lang/types`** — three new public types added in v0.2 to support
  the editor + viewer surfaces; existing types unchanged (strict semver).
- **`@fossil-lang/resolvers`** — no API changes; carried along the linked
  bump so the version matrix stays unified.
- **`@fossil-lang/examples`** — fixture set expanded; the canonical
  `hello.fossil` walking-skeleton example is unchanged.
- **`@fossil-lang/editor` — NEW.** Standalone CodeMirror 6 host with
  pluggable `LspTransport` (`HttpTransport` / `WorkerTransport` /
  `DirectTransport`). Extracted from playground-internal in Phase 11;
  `HttpTransport` proven in Keasy via Phase 16 (JSON-RPC over POST to a
  Keasy backend route, per-doc text cache, didChange-response diagnostic
  piggy-backing).
- **`@fossil-lang/viewer` — NEW.** Cosmos.gl-backed WebGL graph viewer +
  Turtle/Vertices/Edges tabs + light/dark theming. Ported from in-tree
  Keasy code in Phase 12; proven against three Keasy consumer pages
  (discover/page.tsx, jobs/detail/discovery-view.tsx, catalog-view.tsx)
  in Phase 16.

### Compatibility

(and similar for the other five pre-existing packages) — **no breaking
changes on the 6 pre-existing packages**. The bump is strictly additive +
bugfixes per the v0.2 milestone semver discipline. Adopters who want to
pull in the new `editor` + `viewer` packages should follow `MIGRATION-v0.2.md`
(REL-02, ships alongside this release).

Internal Keasy consumption flips from the pre-publish `file:./vendor/fossil-lang/*.tgz`
shim (per ADR-0038 §Compliance) to registry-resolved `^0.2.0` in this same
release wave. The cross-repo consumption mechanism is documented in
**`decisions/0038-cross-repo-consumption.md`**.

### References

- `MIGRATION-v0.2.md` — REL-02 migration guide (ships alongside this
  release; covers the editor + viewer adoption recipe + transport decision
  tree + Phase 16 troubleshooting).
- `decisions/0038-cross-repo-consumption.md` — ADR-0038, cross-repo
  consumption strategy. Superseded by this release; documented for
  historical context and to explain the `pnpm.overrides` block that
  downstream Keasy-pattern consumers may still encounter in their pre-0.2
  state.
