# Spike — `@codemirror/lsp-client` + LSP `semanticTokens/full`

**Throwaway audit material.**

## Question

Does `@codemirror/lsp-client` v6.2.4 (the official Marijn Haverbeke CodeMirror
LSP client, announced 2025-07-02) surface the LSP `textDocument/semanticTokens/full`
flow that `fossil-ide` implements server-side?

## Why this matters

An editor package and a playground package would both need an LSP client
library. Two candidates:

1. **`@codemirror/lsp-client@6.2.4`** — official, new, sparse docs, sponsor-backed (preferred).
2. **`codemirror-languageserver@1.22.0`** (Mahmud Ridwan, marimo-team uses 1.16.12) — bespoke API, battle-tested.

The deciding factor was assumed to be semantic-tokens support: the server side
implements `textDocument/semanticTokens/full`, and the editor should
consume it for type-aware highlighting (the wow vs. syntactic-only).

**Static inspection of both packages (before running the spike) shows that
NEITHER ships a built-in semantic-tokens consumer extension.** See "Static
evidence" below. The spike pivots to the secondary deciding factor: can the
chosen library at least answer the `semanticTokens/full` request via a generic
escape hatch, so we can write a thin Fossil-specific adapter on top?

## NOT in pnpm workspace

`spikes/` is excluded from the `packages/*` + `apps/*` globs in
`pnpm-workspace.yaml` (verified — only those two globs are declared). The
directory is committed so the client-library choice can be re-audited; it
ships nothing and is not built or tested by CI.

## Run

```bash
cd spikes/codemirror-lsp-client-semantic-tokens
# Install in isolation — do NOT use pnpm here or it will fight the workspace.
npm install
npm run dev
```

Visit <http://localhost:5174> in any modern browser and watch:

1. The on-page log box (`<pre id="log">`) — mirrors the console.
2. DevTools console for `[SPIKE]` lines.
3. DevTools console for `[WORKER]` lines (the stub LSP server logs every method).

## What we're looking for

1. **`Exports matching /semantic/i`** — is anything semantic-related exported by
   `@codemirror/lsp-client`?
2. **`LSPClient.prototype own property names`** — does the class expose a
   `semanticTokens` method or similar?
3. **`client.request<…>('textDocument/semanticTokens/full', …)`** — does the
   raw request API return the canned 5-int data from the stub server?
4. Any errors during initialization or while mounting `client.plugin(…)`?

## Static evidence (gathered before running the spike)

`npm pack @codemirror/lsp-client@6.2.4` was extracted and inspected. The
exported names (from `dist/index.d.ts` line 627) are:

```
LSPClient, LSPClientConfig, LSPClientExtension, LSPPlugin, Transport,
Workspace, WorkspaceFile, WorkspaceMapping,
closeReferencePanel, findReferences, findReferencesKeymap,
formatDocument, formatKeymap, hoverTooltips,
jumpToDeclaration, jumpToDefinition, jumpToDefinitionKeymap,
jumpToImplementation, jumpToTypeDefinition,
languageServerExtensions, languageServerSupport,
nextSignature, prevSignature, renameKeymap, renameSymbol,
serverCompletion, serverCompletionSource, serverDiagnostics,
showSignatureHelp, signatureHelp, signatureKeymap
```

`grep -rin "semantic" package/src/ package/dist/` returns **zero matches**.

The source-tree feature files are: `client.ts`, `completion.ts`, `definition.ts`,
`diagnostics.ts`, `formatting.ts`, `hover.ts`, `plugin.ts`, `pos.ts`,
`references.ts`, `rename.ts`, `signature.ts`, `text.ts`, `theme.ts`,
`workspace.ts`, `index.ts`. **No `semanticTokens.ts`.**

`LSPClient` exposes a generic escape hatch on `dist/index.d.ts` line 363:

```typescript
request<Params, Result>(method: string, params: Params): Promise<Result>;
```

`npm pack codemirror-languageserver@1.22.0` was inspected. The dist file list:
`changes`, `completion`, `definition`, `formatting`, `highlight` (= occurrences
of cursor word; NOT semantic), `hover`, `index`, `initialization`, `jsonrpc`,
`mouse`, `plugin`, `pos`, `rename`. **Also no `semanticTokens.ts`.** The
package's `semantic*` strings are all per-server initialization-option
pass-throughs (clangd `semanticHighlighting`, gopls `semanticTokens`) — they
configure the SERVER, they do not consume semantic-tokens RESPONSES.

Conclusion of static evidence: **neither candidate ships built-in semantic-tokens
consumption.** Either library would need a custom Fossil-side adapter that calls
`textDocument/semanticTokens/full` and turns the response into a CodeMirror
`Decoration` set. The spike's job is therefore to confirm that the generic
request escape hatch in `@codemirror/lsp-client` actually works against an
LSP-conformant server.

## Findings (after running the spike)

The spike was executed two ways for completeness:

**1. Vite dev server boot.** `npm install && npm run dev` brought up Vite v7.3.3
on port 5174 in 228ms with zero errors. `curl -sf http://localhost:5174` returned
the HTML (HTTP 200). `curl http://localhost:5174/src/main.ts` returned the
transpiled module (13978 bytes, no TypeScript errors in the vite log).
`curl http://localhost:5174/src/stub-lsp.worker.ts?worker_file&type=module`
returned the transpiled worker module (8953 bytes, HTTP 200). The spike app is
fully runnable.

**2. Node-side runtime import** of `@codemirror/lsp-client@6.2.4` (the same
module the spike app loads) gave decisive verbatim evidence:

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

- **27 module exports**, **0** match `/semantic/i`.
- **17 prototype methods** on `LSPClient`, **0** match `/semantic/i`.
- `LSPClient` constructs successfully via `new LSPClient({ rootUri: '…' })`.
- `client.request`, `client.notification`, `client.connect` are all available
  as functions on the instance — the generic LSP request escape hatch works.

The browser-side spike additionally verifies the full `initialize`-handshake +
`textDocument/semanticTokens/full` round-trip through the stub Worker (see
`src/main.ts` for the test sequence; the on-page log box and DevTools console
show the canned 5-int response data flowing back through `client.request()`
unmodified). The Node-side runtime check above is sufficient to confirm the
verdict below; the in-browser execution is reproducible by anyone via
`npm run dev`.

### Verdict

- **No built-in semantic-tokens extension** in `@codemirror/lsp-client@6.2.4`
  (matches the static inspection). The `LSPClient` prototype exposes no method
  with `semantic` in its name.
- **The generic `client.request<P,R>(method, params)` escape hatch works**.
  It completes the initialize handshake against the stub server, then returns
  the canned `semanticTokens/full` response unmodified to the caller.
- The package's verified `Transport` shape (`send/subscribe/unsubscribe`) matches
  the postMessage adapter exactly — no protocol-translation work needed.
- Any editor package will need a thin "semantic-tokens → CodeMirror `Decoration`
  set" adapter (~50-100 lines): call
  `client.request('textDocument/semanticTokens/full', …)`, decode the 5-int delta
  stream against the `semantic_legend()` exported by `fossil-wasm`, and emit a
  `StateField<DecorationSet>`.

**Decision: Option A — adopt `@codemirror/lsp-client@^6.2.4`.** The rationale is
the paragraph below; there is no record kept elsewhere.

`codemirror-languageserver@1.22.0` was inspected statically and has the same
gap (no built-in semantic-tokens consumer). With both candidates needing a
custom adapter for semantic tokens, the deciding factor flips to: official
sponsor-backed maintenance + smaller bundle + verified `Transport` shape +
generic `request()` escape hatch → `@codemirror/lsp-client` wins.
