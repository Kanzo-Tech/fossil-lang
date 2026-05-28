---
"@fossil-lang/editor": patch
"@fossil-lang/playground": patch
---

_Bump-level downgraded from `minor` to `patch` as part of Phase 17 REL-01 release squash (this changeset's narrative is preserved; the v0.2.0 minor bump is carried by `release-v0-2-0.md`)._

Phase 11 — `@fossil-lang/editor` extraction.

Editor componente standalone publicable + transport LSP pluggable:

- **`@fossil-lang/editor` is a NEW workspace package** (EDIT-01) at the
  `0.2.0` initial release. Exposes `<FossilEditor/>` (CodeMirror 6 +
  `@fossil-lang/codemirror-fossil` lexer + LSP wiring) + the three locked
  transport implementations: `WorkerTransport` (browser WASM LSP),
  `HttpTransport` (POST JSON-RPC with `AbortSignal` support — Keasy backend
  integration), and `NullTransport` (read-only static). Per ADR-0028
  (multi-host React library family) + ADR-0032 (`@codemirror/lsp-client`
  semantic tokens) + ADR-0036 (Transport-superset design).
- **Transport interface pluggable** (EDIT-02) — `lspTransport: Transport |
  null` prop accepts any of the three shipped implementations OR a host-
  supplied custom transport implementing the `Transport` interface
  exported from the package. Cancellation is opt-in via
  `SendOptions.signal` (HTTP honours; Worker documents the ignore).
- **UX paridad across transports** (EDIT-03) — hover, completion,
  diagnostics, and semantic-tokens render with identical anatomy in all
  three modes. Verified by `apps/landing/tests/e2e/editor.spec.ts`
  (8-test Playwright spec across two fixture pages — Worker + HTTP on
  `/editor.html`, NullTransport isolated on `/editor-null.html` on the
  same `:4173` vite-preview server — the same multi-host-fixture pattern
  shipped in Phase 10's `/primitives.html` oracle).
- **`@connector/path` autocomplete** preserved — `resolver: ConnectionResolver`
  prop is the same 2-tier shape from CONN-01..03 (carryover v0.1). Phase
  11 11-04 spec includes a dedicated test asserting the resolver-driven
  autocomplete popup renders when the user types `@`.
- **`@fossil-lang/playground` re-exports `FossilEditor` and
  `createWorkerTransport` from `@fossil-lang/editor`** — v0.1.x consumers
  using `import { FossilEditor } from '@fossil-lang/playground'` or
  `import { createWorkerTransport } from '@fossil-lang/playground'` do NOT
  break. Module-instance dedup via pnpm workspace symlinks.
- **Bundle budget** — `@fossil-lang/editor` ships at 1.31 KB gzipped (well
  under the 100 KB cap per Phase 11 CONTEXT.md — CM6 + LSP-client are
  peerDeps so they don't count against the editor's own surface);
  `@fossil-lang/playground` is 95.75 KB (was 95.53 KB Phase 10 baseline;
  +0.22 KB delta, well under its 500 KB cap).

Theming: the editor is theme-less; consumes the `--fossil-*` CSS variable
contract via the host's theme provider. Kanzo-branded hosts install
`@kanzo/theme` and wrap in `<KanzoThemeProvider/>` (per ADR-0035 visual
ownership separation, Phase 10).

No Rust changes — WASM 9-crate gate + walking-skeleton invariant intact
throughout the five plans. ADRs referenced: 0026 (Worker lifecycle), 0028
(multi-host library family), 0029 (ConnectionResolver), 0030 (`tokenize`
export shape), 0032 (`@codemirror/lsp-client`), 0033 (`@fossil-lang/ui`),
0035 (visual ownership separation), 0036 (Transport-superset — NEW in
Phase 11 plan 11-01).
