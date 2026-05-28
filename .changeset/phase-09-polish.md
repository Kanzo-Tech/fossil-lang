---
"@fossil-lang/playground": patch
"@fossil-lang/codemirror-fossil": patch
"@fossil-lang/wasm": patch
"@fossil-lang/types": patch
"@fossil-lang/resolvers": patch
"@fossil-lang/examples": patch
---

_Bump-level downgraded from `minor` to `patch` as part of Phase 17 REL-01 release squash (this changeset's narrative is preserved; the v0.2.0 minor bump is carried by `release-v0-2-0.md`)._

Phase 9 — playground polish + differentiators.

Eight new features that turn the v0.1 React library into a demo-ready
playground for the KGC community + ESWC/ISWC paper drafts:

- **Stateless permalinks** (PLAY-04) — `encodePermalink` / `decodePermalink`
  with `fflate` gzip + base64url + a schema-versioned envelope
  (`{ v: 1, source, csvw?, shex? }`). `<FossilPlayground/>` accepts
  `initialPermalink` and emits `onStateChange`; CI-verified forward-compat
  fixtures (`tests/fixtures/permalink-fixtures.json`) guarantee any future
  v0.2 schema preserves v1 URL decodability — the "paper permanence" gate.
- **Curated examples** (PLAY-05) — six bundled examples (`hello`,
  `hello-no-csvw`, `ecommerce`, `musicbrainz`, `typing-showcase`,
  `multi-source-join`) each with a 4-mutation variations harness
  (30 compile-outcome checks per PR via the new
  `.github/workflows/examples.yml` gate). Native fossil-cli drives the
  harness — no JSDOM/WASM polyfill stack.
- **Landing page hero** (PLAY-06) — `<LandingHero/>` Server Component with
  tagline, "Try the 10-second example" CTA, pipeline diagram, GitHub repo
  link, Min Oo & Hartig (ESWC 2025) foundational-paper citation, and the
  Hartig BibTeX entry in a collapsible `<details>` for paper drafts.
- **View Compiled SQL panel** (PLAY-07) — `CompiledSqlPanel` (read-only
  CodeMirror 6) with a custom DuckDB `SQLDialect` extending StandardSQL
  (PIVOT/UNPIVOT/QUALIFY/asof/list_aggr/regexp_extract/etc.) — live-updates
  as the user edits the mapping. The single most-tested feature in the
  launch demo and the visible differentiator vs RMLMapper / Morph-KGC.
- **BibTeX cite modal** (PLAY-08) — `BibtexModal` (native `<dialog>` —
  focus trap + Escape close + role/aria-modal from the platform) embeds
  the playground permalink plus the Min Oo & Hartig reference. Cite key
  uses the first 8 alphanumeric chars of the permalink (collision-free
  for paper-scale cite counts).
- **CSVW inferred preview** (PLAY-09) — `CsvwPreview` editable form
  (column name + datatype `<select>` + drop column) with a dirty-state
  guard (RESEARCH.md Pitfall 4) so user edits are never clobbered by
  background re-inference. Reset-to-inferred restores the auto-inferred
  descriptor + clears the dirty flag.
- **Turtle view tab** (PLAY-10) — `TurtleTab` renders materialized
  vertex/edge tables as Turtle via `n3.Writer`; xsd:integer / xsd:double
  / xsd:boolean datatype derivation; ARIA tabpanel + accessible Copy
  button. Result panel is now a tablist of three tabs (Graph / Edges
  / Turtle) using native button[role=tab] (no Radix dependency).
- **Automatic CSVW inference** (PLAY-11) — empty descriptor + `io.csv("...")`
  source triggers `inferCsvw(conn, csvUrl)` via DuckDB-WASM's
  `DESCRIBE SELECT * FROM read_csv_auto(...)`; transient connection is
  closed in the finally so we never leak; failures are non-fatal (user can
  still hand-type). The "10 seconds to a triple" promise without forcing
  CSVW upfront.

Carry-forward closures:

- **CODEGEN-LOWERING-01** — `crates/fossil-mir` `Op::TripleEmit` lowering
  emits `Expr::ColRef { source: "", column }` so `render_expr`'s
  `default_source` (view name) substitutes correctly. Walking-skeleton
  invariant preserved (`fossil compile examples/hello.fossil` produces
  5 triples).
- **SW-FIRST-LOAD-01** — `apps/landing/app/ClientShell.tsx` adds the
  `controllerchange` → `location.reload()` listener that pairs with the
  Phase 8 Serwist `skipWaiting()` + `clients.claim()` (RESEARCH.md
  Pitfall 3); `offline.spec.ts:98` re-enabled + new
  `sw-multitab.spec.ts` proves multi-tab SW convergence.
- **REQ-TRACE-01** — REQUIREMENTS.md traceability sweep: 8 Phase-9
  PLAY-* IDs marked Complete (PLAY-04..11); Phase 8 CONN/PKG/A11Y/
  THEME/OFFLINE entries confirmed Complete.

Architecture & invariants preserved:

- 9-crate WASM gate green (`cargo check --target wasm32-unknown-unknown`
  on `fossil-base/syntax/hir/mir/codegen/sinks/ide/ide-db/wasm`).
- `MAX_PER_MAPPING_FAN_OUT=1` invariant intact.
- Walking-skeleton intact (5-triple parquet from `examples/hello.fossil`).
- Bundle budgets honored: `@fossil-lang/playground` core <500 KB gzip;
  WASM <2 MB compressed; landing build <10 MB.

No new ADRs landed in Phase 9 — all decisions covered by ADRs 0024–0032
(Phase 7/8 carry-forward set).

See `.planning/phases/09-playground-polish-differentiators/SUMMARY.md`
for the full phase close with per-SC evidence table.
