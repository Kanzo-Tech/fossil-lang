# ADR 0027: Pivot editor to CodeMirror 6 (away from Monaco)

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/research/playground-sources-data-tools.md` (data tooling research synthesis)
- `.planning/research/playground-sources-rdf-tools.md` §"Editor stacks across KG tools"
- `.planning/research/playground-sources-design.md` §4 "Frontend stack"
- Keasy parts inventory (mid-conversation Explore agent, 2026-05-24) — Keasy uses CodeMirror 6 in `web/src/components/discovery/code-editor.tsx` with a Fossil `StreamLanguage` + `@`-autocomplete
- ADR-0028 (React library shape — context for editor pick)
- ADR-0030 (WASM-exported tokenizer — orthogonal to editor, enabled by it)
- Supersedes the Phase-7 plan 07-05 choice (monaco-editor 0.52.0 + monaco-languageclient 10.7.0)

## Context

Phase 7 plan 07-05 picked Monaco + `monaco-languageclient` v10.7. The work
landed (~2 days of wiring: Monaco mount in 4 panels, .fossil + shex language
registrations, semantic-tokens legend mirror, build-wasm-worker script). The
LSP-side investment from Phase 6 + plans 07-01/02/03 (fossil-wasm Workspace
API + LSP-over-postMessage dispatch + per-file diagnostics drain) is editor-
agnostic and stays valid regardless of the client editor.

Two facts surfaced during the 2026-05-24 redesign conversation that retroactively
invalidate the Monaco pick:

1. **Keasy already uses CodeMirror 6** for its Fossil script editor (the
   playground we're rebuilding ships *into* Keasy as a host). Keeping Monaco
   in the playground would force Keasy to either run two editor stacks in
   the same app (Monaco for the playground component + CodeMirror everywhere
   else) or rip out its existing editor for parity. Both options are worse
   than aligning the playground with the dominant host.

2. **The component is a reusable React library** (ADR-0028) shipped to npm,
   not a standalone Vite app. Bundle size becomes a public cost paid by every
   consumer. Monaco's runtime is ~3 MB; CodeMirror 6 is ~150–300 KB depending
   on extensions. For a component embedded into landing pages, docs sites,
   and Keasy, a 10× bundle factor matters.

LSP-via-CodeMirror is well-supported via `codemirror-languageserver` (or a
thin custom adapter); the LSP JSON-RPC + postMessage dispatch (07-03) is
unaffected — the same Worker speaks to either client.

A11y: CodeMirror 6's screen-reader, keyboard nav, and focus-management story
is on par with Monaco for the WCAG 2.1 AA target.

## Decision

The Fossil playground component uses **CodeMirror 6** as its editor. The
Monaco + monaco-languageclient wiring from plan 07-05 is discarded.

The LSP transport (07-03 dispatch + 07-02 Workspace API) is retained
verbatim; the client-side adapter changes from `monaco-languageclient`
to `codemirror-languageserver` (or a custom adapter if the package needs
patching for our message shape).

Syntactic highlighting comes from a `StreamParser` that delegates to a
`tokenize(text)` function exported from `fossil-wasm` (see ADR-0030);
semantic highlighting comes from the LSP `semanticTokens` request that
06-07 already implements.

## Consequences

**Positive.**

- Bundle savings: ~3 MB → ~300 KB editor footprint. Real for every host.
- Stack parity with Keasy → easier downstream migration, single editor
  contract across the org.
- CodeMirror 6's tree-shakeable architecture lets us ship only the extensions
  we use (no Monaco-style monolith).
- Native React integration via `@uiw/react-codemirror` (or hand-rolled
  thin wrapper) — no `vscode/`-namespace shim layer required.

**Negative.**

- Plan 07-05's ~2 days of Monaco wiring is sunk cost. The `playground/src/`
  Monaco mount code is discarded (the whole `playground/` tree is being
  restructured into `packages/` per ADR-0031, so the discard happens as
  part of a larger reorganisation).
- Less out-of-the-box LSP polish: Monaco + `monaco-languageclient` ships
  more behaviours pre-wired (signature help UX, inlay hint rendering,
  cancellation tokens) — for CodeMirror we will wire the same features
  manually as we need them.
- VS-Code-like aesthetic is partially lost. The audience (KGC community,
  data engineers) is comfortable with both editors; we estimate the
  preference cost as small.

**Neutral.**

- Phase 9's VS Code extension is unaffected — that extension uses the
  full VS Code Monaco automatically; the playground's editor choice does
  not constrain it.
- ADR-0024's "one crate, two hosts" model now becomes "one crate, three
  hosts" once Phase 9 lands: `fossil-lsp` native stdio, `fossil-wasm`
  WASM postMessage to CodeMirror, and the VS Code extension's Monaco.
  The seam is the same.

## Alternatives considered

1. **Keep Monaco for the playground despite Keasy's CodeMirror.** Rejected.
   Bundle cost is real, Keasy parity is lost, dual-editor maintenance in
   Keasy is a recurring tax.

2. **Headless editor abstraction** — expose an `<EditorBackend>` slot;
   ship both CodeMirror and Monaco implementations. Rejected as
   premature: doubles the integration surface (two LSP wirings, two
   highlighting paths, two theme APIs) for a feature no consumer has
   asked for.

3. **Defer the choice; ship the playground without an editor opinion
   and let hosts pick.** Rejected — an editor IS the playground; the
   component cannot ship without one.
