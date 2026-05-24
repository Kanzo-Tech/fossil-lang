# ADR 0032: Adopt `@codemirror/lsp-client` with a thin Fossil-side semantic-tokens adapter

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/phases/08-playground-react-library-v0-1/08-RESEARCH.md` — "Key finding 1: `@codemirror/lsp-client` is the new official LSP client (announced 2025-07)" + Open Question 1 + Pitfall 2
- `spikes/codemirror-lsp-client-semantic-tokens/` — Wave-0 spike app + verbatim runtime evidence in its `README.md` "Findings" section
- `decisions/0027-codemirror-over-monaco.md` — the editor pivot that triggered the LSP-client question
- `decisions/0030-wasm-exported-tokenizer.md` — single grammar source of truth (provides `semantic_legend()`, the legend the adapter consumes)
- `decisions/0024-fossil-wasm-workspace-api.md` — the LSP server side this client talks to over postMessage
- `@codemirror/lsp-client@6.2.4` published 2026-05-15 (initial 6.0.0 release announced 2025-07-02 by Marijn Haverbeke); `dist/index.d.ts` line 627 for the verified export list; line 363 for the generic `request<Params, Result>` escape hatch
- `codemirror-languageserver@1.22.0` (Mahmud Ridwan; used at v1.16.12 by marimo-team) — fallback candidate inspected for comparison

## Context

Plan 08-08 (`@fossil-lang/codemirror-fossil`) and plan 08-09 (`@fossil-lang/playground`) both depend on a CodeMirror 6 LSP client library to consume the Phase 6 + Phase 7 LSP server delivered through `fossil-wasm`. ADR-0027's pivot from Monaco to CodeMirror picked the editor; the LSP-client library was left open with two candidates:

1. **`@codemirror/lsp-client@^6.2.4`** — the official Marijn Haverbeke client. Initial 6.0.0 released 2025-07-02; the most recent version at the time of this ADR is 6.2.4 (2026-05-15). Sponsor-backed, dependency-light, minimal API surface.
2. **`codemirror-languageserver@^1.22.0`** (formerly `@marimo-team/codemirror-languageserver`) — Mahmud Ridwan's bespoke client. Battle-tested against TypeScript Server, Python, clangd, gopls; actively maintained; marimo-team is a notable production user at v1.16.12 (Feb 2026).

The deciding factor was assumed to be semantic-tokens support: Phase 6 plan 06-07 implements LSP `textDocument/semanticTokens/full` server-side, the editor must consume it to enable type-aware highlighting (the "wow vs. syntactic-only" pitch of the playground). RESEARCH.md Open Question 1 flagged this as the highest-uncertainty unknown in Phase 8 and recommended a Wave-0 spike. Without it, plan 08-08 risks adopting a library that needs a fork on day 3.

The spike at `spikes/codemirror-lsp-client-semantic-tokens/` does three things:

1. Statically inspects both packages (`npm pack` + grep for `semantic`) for any built-in semantic-tokens consumer extension.
2. At runtime, imports `@codemirror/lsp-client@6.2.4` from Node and confirms its module exports + `LSPClient` prototype shape.
3. In a browser via Vite, connects `@codemirror/lsp-client` to a stub LSP Worker that returns a canned `semanticTokens/full` 5-int response, and verifies the response round-trips through the generic `client.request()` escape hatch.

## Spike findings

**Static (both libraries inspected via `npm pack`):**

- `@codemirror/lsp-client@6.2.4` `dist/index.d.ts` line 627 enumerates exactly 27 exports: `LSPClient`, `LSPPlugin`, `Workspace`, `WorkspaceMapping`, plus completion/diagnostics/hover/definition/references/rename/signature/formatting feature extensions. **No `semantic*` export.** `grep -rin "semantic" package/src/ package/dist/` returns zero matches.
- `codemirror-languageserver@1.22.0` `dist/` lists `changes`, `completion`, `definition`, `formatting`, `highlight` (= occurrences-of-cursor, NOT semantic), `hover`, `mouse`, `plugin`, `rename`, `index`, `initialization`, `jsonrpc`, `pos`. **No semantic-tokens consumer.** The `semantic*` strings found in `initialization.d.ts` are per-server initialization-option pass-throughs (clangd `semanticHighlighting`, gopls `semanticTokens`) — they configure the SERVER, they do not consume semantic-tokens RESPONSES.

**Conclusion of static evidence:** Neither candidate ships a built-in semantic-tokens consumer. Either library would need a Fossil-side adapter that calls `textDocument/semanticTokens/full` and turns the response into a CodeMirror `Decoration` set.

**Node-side runtime probe** (verbatim, copy-pasted from the spike Bash session, see also `spikes/codemirror-lsp-client-semantic-tokens/README.md` "Findings"):

```
EXPORTS: ["LSPClient","LSPPlugin","Workspace","WorkspaceMapping",
          "closeReferencePanel","findReferences","findReferencesKeymap",
          "formatDocument","formatKeymap","hoverTooltips",
          "jumpToDeclaration","jumpToDefinition","jumpToDefinitionKeymap",
          "jumpToImplementation","jumpToTypeDefinition",
          "languageServerExtensions","languageServerSupport",
          "nextSignature","prevSignature","renameKeymap","renameSymbol",
          "serverCompletion","serverCompletionSource","serverDiagnostics",
          "showSignatureHelp","signatureHelp","signatureKeymap"]
SEMANTIC_MATCHES: []
LSPClient_typeof: function
LSPClient_proto_keys: ["cancelRequest","connect","connected","constructor",
                       "didClose","didOpen","disconnect","hasCapability",
                       "notification","plugin","receiveMessage","request",
                       "requestInner","sync","timeoutRequest","withMapping",
                       "workspaceMapping"]
LSPClient_proto_semantic_matches: []
LSPClient_constructed_ok: object
client_has_request: function
client_has_notification: function
client_has_connect: function
```

27 exports, 0 match `/semantic/i`. 17 prototype methods on `LSPClient`, 0 match `/semantic/i`. The generic `request<Params, Result>(method, params): Promise<Result>` escape hatch IS exposed and IS callable — it is the path through which semantic tokens (and any other LSP method this library does not natively wrap) can be sent.

**Browser-side runtime probe** (Vite dev server at port 5174, stub Worker LSP):

- Vite v7.3.3 boots in 228ms with zero TypeScript or transpile errors.
- `LSPClient` instantiates via `new LSPClient({ rootUri: 'file:///spike' })` and accepts `.connect(transport)` with the verified `Transport` shape `{ send(s), subscribe(h), unsubscribe(h) }` from `dist/index.d.ts` line 170.
- The stub Worker advertises `semanticTokensProvider` in its `initialize` response. `client.serverCapabilities.semanticTokensProvider` is populated after `client.initializing` resolves.
- `await client.request<…>('textDocument/semanticTokens/full', { textDocument: { uri } })` returns the canned `{ data: [0,0,6,0,0, 0,7,2,4,0] }` payload unmodified — the client neither rejects, nor reshapes, nor strips the response.

## Decision

**Adopt `@codemirror/lsp-client@^6.2.4`** as the LSP client library across `@fossil-lang/codemirror-fossil` (plan 08-08) and `@fossil-lang/playground` (plan 08-09). Both packages declare it as a peerDependency so consumer hosts can dedupe.

Semantic tokens are NOT a built-in feature of `@codemirror/lsp-client@6.2.4`. Plan 08-08 implements a thin Fossil-side adapter, expected size 50–100 lines, in `packages/codemirror-fossil/src/semantic-tokens.ts`:

```typescript
// Sketch — full design lands in plan 08-08
export function fossilSemanticTokens(client: LSPClient, uri: string): Extension {
  return ViewPlugin.fromClass(class {
    decorations = Decoration.none;
    constructor(view: EditorView) { this.update(view); }
    async update(view: EditorView) {
      const resp = await client.request<
        { textDocument: { uri: string } },
        { data: number[] } | null
      >('textDocument/semanticTokens/full', { textDocument: { uri } });
      if (!resp) return;
      // Decode 5-int delta stream against fossil-wasm's semantic_legend() (08-02)
      // and emit a DecorationSet keyed on tokenType -> highlight tag.
      this.decorations = decodeDelta(resp.data, semanticLegend);
    }
  }, { decorations: v => v.decorations });
}
```

The legend mapping is provided by `semantic_legend()` exported from `fossil-wasm` (Phase 8 plan 08-02 — already shipped, commit `8358acc`), which itself re-exports `fossil_ide::semantic_legend` from Phase 6 plan 06-07. The single grammar source of truth (ADR-0030) flows uninterrupted from `fossil-syntax/lexer.rs` through `fossil-ide/semantic.rs` through `fossil-wasm` into the editor.

`codemirror-languageserver` is NOT adopted as the primary, but is preserved as a documented fallback: if a future change to `@codemirror/lsp-client` removes the generic `request()` escape hatch (a remote possibility given the package's stated intent of "minimal API surface"), 08-08's thin adapter can be re-pointed at `codemirror-languageserver`'s equivalent transport in <1 day. The Fossil-side semantic-tokens decoder is library-agnostic; only the wire-call site changes.

## Consequences

**Positive.**

- **Smaller install footprint.** `@codemirror/lsp-client` has only 8 runtime dependencies (`marked`, `@codemirror/{autocomplete,language,lint,state,view}`, `@lezer/highlight`, `vscode-languageserver-protocol`). `codemirror-languageserver`'s tree is larger because of its per-server feature modules.
- **Stack alignment with official CodeMirror.** Future hover/completion/diagnostics UX improvements landing in `@codemirror/lsp-client` flow to Fossil for free. Sponsor-backed maintenance ≈ multi-year horizon.
- **Verified `Transport` shape** matches our postMessage adapter exactly (`send/subscribe/unsubscribe` — no protocol-translation work). The fossil-wasm LSP worker dispatch from Phase 7 (plans 07-02/03) is editor-agnostic and stays unchanged.
- **Single grammar source of truth (ADR-0030) extends naturally** into the editor — the semantic-tokens adapter consumes `fossil-wasm`'s `semantic_legend()` (06-07 → 08-02), so the highlight category table has no TS-side duplicate to drift.
- **Future-proof on the upstream side:** if `@codemirror/lsp-client` ships a built-in semantic-tokens extension later (the package has been evolving rapidly — 6.0.0 to 6.2.4 in 10 months), our thin adapter is delete-able, the call site moves to the official extension, and the Fossil legend mapping is reused.

**Negative.**

- **We own a Fossil-side semantic-tokens adapter (50–100 lines).** This is real maintenance: any change to the LSP `semanticTokens` v3.17 protocol (deltas, range requests) requires updating the decoder. Mitigation: scope the v0.1 adapter to `semanticTokens/full` ONLY (no `range` or `delta` requests); revisit when 06-07 server-side adds them.
- **Sparse docs on `@codemirror/lsp-client`.** README + d.ts are the canonical sources; no extensive examples or recipes beyond the `test/webtest-client.ts` file in the package. Mitigation: 08-08's adapter and its docstrings serve as the Fossil-internal reference recipe.
- **One more thing-we-built-instead-of-using-a-library.** Generic LSP semantic-tokens consumers exist in `monaco-languageclient` (which we just abandoned per ADR-0027) and in some VS Code internals; neither is reusable for CodeMirror. Mitigation: this is intrinsic to the CodeMirror ecosystem at this point in time, not a Fossil-specific cost.

**Neutral.**

- The `@codemirror/lsp-client` `LSPClient` constructor takes `LSPClientConfig?` then `.connect(transport)` separately (verified — `dist/index.d.ts` lines 282-320). This is slightly different from the conventional "pass transport into the constructor" pattern, but the spike confirms it works as documented.
- `LSPClient.serverCapabilities` exposes the parsed `lsp.ServerCapabilities` from the server's `initialize` response — so the adapter can gate on `serverCapabilities.semanticTokensProvider` before making the request, avoiding errors against servers that don't support it.
- Phase 9's VS Code extension is unaffected — that extension uses VS Code's built-in Monaco-based LSP plumbing.

## Alternatives considered

1. **Adopt `codemirror-languageserver@^1.22.0` as the primary client.** Rejected. Static inspection shows it has the same semantic-tokens gap (no built-in consumer; the `semantic*` strings in its source are server-config pass-throughs). With both candidates needing a Fossil-side adapter for semantic tokens, the deciding factor flips to: official sponsor-backed maintenance + smaller bundle + verified `Transport` shape match + generic `request()` escape hatch → `@codemirror/lsp-client` wins. Kept as a documented fallback in case `@codemirror/lsp-client` ever removes its generic `request()`.

2. **Write our own thin LSP client over postMessage from scratch.** Rejected. The 8-dependency surface of `@codemirror/lsp-client` includes value we'd otherwise have to re-implement: completion/hover/diagnostics UX widgets, signature help, references panel, rename refactor flow, JSON-RPC framing, request-cancellation semantics, server-capability gating. The spike's `Transport` adapter is ~20 lines; the corresponding "build it ourselves" surface is ~2000 lines.

3. **Defer the editor LSP wiring and ship a syntax-only playground in v0.1.** Rejected. The "wow" of the playground IS the type-aware highlighting + diagnostics. Phase 6's `semanticTokens/full` server is shipped; not consuming it is a step backwards from the Phase 7 plan 07-05 (Monaco) state. Plan 08-09's success criteria explicitly require LSP diagnostics surfacing in the editor.

4. **Wait for `@codemirror/lsp-client` to ship a built-in semantic-tokens extension upstream.** Rejected. The package's release cadence (6.0.0 → 6.2.4 over 10 months, ~4 feature releases) suggests semantic tokens may land eventually, but there is no public roadmap and no issue tracking it. We cannot block Phase 8 on an unscheduled upstream feature.

5. **Fork `@codemirror/lsp-client` and add semantic tokens.** Rejected as premature. A thin 50–100-line Fossil-side adapter via the public `client.request()` API achieves the same outcome without a fork, without divergence from upstream, and without taking on the maintenance burden of an internal package fork. Revisit only if `client.request()` is ever removed (low probability).
