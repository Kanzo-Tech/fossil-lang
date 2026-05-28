---
---

Phase 16 of v0.2 milestone — Keasy migration validated the multi-host pattern
in production. **No `@fossil-lang/*` package versions bumped in this phase**
— Phase 16 is consumer-side; Phase 17 REL-01 will ship `@fossil-lang/* v0.2.0`
to npm.

What landed (across the Keasy repo, branch `fossil-migration`):

- **MIG-04 — CSS bridge.** Keasy `web/src/app/globals.css` adds a 67-line
  additive `--fossil-* → var(<shadcn-token>)` alias block inside `:root`,
  bridging the 26 token names actually consumed by published
  `@fossil-lang/{ui,viewer,editor,codemirror-fossil}` (grep-surveyed, not
  ADR-spec-listed). Dark mode propagates via cascade — no `.dark`
  redeclarations needed. Zero existing Keasy tokens modified.

- **MIG-02 — Viewer swap.** Keasy
  `web/src/components/discovery/{graph-view-v2,cosmos-graph}.tsx` DELETED
  (335 LOC removed). `discovery/page.tsx`, `jobs/detail/discovery-view.tsx`,
  and `jobs/detail/catalog-view.tsx` now consume `@fossil-lang/viewer`'s
  GraphCanvas. A new ~138-LOC `useGraphDataRows` adapter materializes
  Keasy's Mosaic/DuckDB UNION-ALL SQL into the viewer's string-id `{id,
  type, label}` row API while preserving zero-copy parquet streaming.

- **MIG-01 + MIG-03 — Editor swap.** Keasy
  `web/src/components/discovery/code-editor.tsx` DELETED (372 LOC removed).
  `web/src/components/jobs/{step-script,assistant-wizard}.tsx` now consume
  `@fossil-lang/editor`'s `<FossilEditor/>` with HttpTransport against a
  NEW Keasy backend route `POST /v1/fossil/lsp` (~421 LOC of new Rust on
  the Keasy server). The route adapts JSON-RPC envelopes to the existing
  per-org `fossil_lsp::AnalysisHost` shared with `/v1/fossil/analyze`, with
  a per-doc text cache (`OrgAnalysisState.docs`) and didChange-response
  diagnostic piggy-backing.

LOC delta (Keasy): −707 LOC of in-tree component code deleted; ~+571 LOC
added (~138 viewer adapter + ~85 step-script rewrite + ~5 assistant rewrite
+ ~440 server-side JSON-RPC route). Net is mildly negative on the web side
(~−480 TS) and additive on the server side (~+440 Rust) — but the
structural win the LOC figures don't capture is that Keasy is now sourcing
from `@fossil-lang/*` instead of carrying duplicates, so the long-tail of
maintenance on three production-grade components moves to the shared
library.

Verification (against Phase 15 baselines):

- Visual gate green — `apps/landing/tests/e2e/visual-baselines.spec.ts`
  re-run against the 20 PNG baselines captured in 15-05: **20/20 cells
  pass** within the 2% pixel-diff tolerance. The Keasy migration did not
  regress the landing's look (proving no leakage into shared library code).
- WASM gate green — `cargo check --target wasm32-unknown-unknown` clean
  across the 9 gated compiler crates (Phase 16 had zero Rust changes in
  the Fossil repo; the gate runs as a structural invariant).
- Walking-skeleton invariant green — `cargo run -p fossil-cli -- compile
  examples/hello.fossil` produces `output.parquet` + `manifest.yaml`.

Cross-repo consumption mechanism (npm pack + `file:` protocol; pre-publish
smoke) is documented in **ADR-0038** (`decisions/0038-cross-repo-consumption.md`).
Phase 17 REL-01 will flip Keasy's `file:./vendor/fossil-lang/*.tgz` deps to
`^0.2.0` after the formal npm publish, at which point the `pnpm.overrides`
shim in `keasy/web/package.json` can also be removed.
