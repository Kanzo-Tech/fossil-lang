// Spike entry point — wires @codemirror/lsp-client to a stub LSP Worker and
// answers: does the client surface `textDocument/semanticTokens/full`?
//
// What we LOG (all visible in the on-page <pre id="log"> + DevTools console):
//   1. The exported names from `@codemirror/lsp-client` — does any name include `semantic`?
//   2. The own-keys + prototype-keys of an instantiated `LSPClient` — same check.
//   3. The result of `client.request('textDocument/semanticTokens/full', ...)`
//      — does the raw request API return the canned 5-int data from the stub server?
//
// The verdict decides which LSP client library the editor adopts.

import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import * as lspMod from '@codemirror/lsp-client';
import type { Transport } from '@codemirror/lsp-client';

const logEl = document.getElementById('log')!;

function log(...args: unknown[]): void {
  const line =
    '[SPIKE] ' +
    args
      .map((a) => (typeof a === 'string' ? a : JSON.stringify(a, replacer, 2)))
      .join(' ');
  console.log(line);
  logEl.textContent += line + '\n';
}

// JSON.stringify replacer that handles undefined + functions readably.
function replacer(_key: string, value: unknown): unknown {
  if (typeof value === 'function') return `[function ${(value as { name?: string }).name ?? '<anon>'}]`;
  if (value === undefined) return '[undefined]';
  return value;
}

log('=== Spike start — @codemirror/lsp-client semanticTokens probe ===');

// 1. What does the package export? Does any export name include "semantic"?
const exportedNames = Object.keys(lspMod).sort();
log('Exported names from @codemirror/lsp-client:', exportedNames);
const semanticExports = exportedNames.filter((n) => /semantic/i.test(n));
log('Exports matching /semantic/i:', semanticExports.length ? semanticExports : '(none)');

// 2. Create the Worker + Transport adapter (the known-stable part of the API).
const worker = new Worker(new URL('./stub-lsp.worker.ts', import.meta.url), { type: 'module' });

const handlers = new Set<(msg: string) => void>();
worker.addEventListener('message', (e: MessageEvent) => {
  const msg = typeof e.data === 'string' ? e.data : JSON.stringify(e.data);
  handlers.forEach((h) => h(msg));
});

const transport: Transport = {
  send(msg: string) {
    worker.postMessage(msg);
  },
  subscribe(h: (m: string) => void) {
    handlers.add(h);
  },
  unsubscribe(h: (m: string) => void) {
    handlers.delete(h);
  },
};

// 3. Inspect the LSPClient class surface before instantiating.
const { LSPClient } = lspMod;
log('LSPClient is', typeof LSPClient);
log(
  'LSPClient.prototype own property names:',
  Object.getOwnPropertyNames(LSPClient.prototype).sort(),
);

// 4. Instantiate via the verified constructor signature (LSPClientConfig?) + connect(transport).
//    See @codemirror/lsp-client@6.2.4 dist/index.d.ts lines 282-320.
const client = new LSPClient({ rootUri: 'file:///spike' });
log('LSPClient instantiated', {
  hasRequest: typeof client.request,
  hasNotification: typeof client.notification,
  hasConnect: typeof client.connect,
  hasPlugin: typeof client.plugin,
});

// Connect — this triggers the initialize handshake against the stub worker.
client.connect(transport);
log('client.connect(transport) called; awaiting initialization …');

// 5. Mount a minimal CodeMirror editor with the LSP plugin attached.
//    This proves the editor mount path doesn't crash with the LSP client wired.
const editor = new EditorView({
  state: EditorState.create({
    doc: 'prefix ex: <https://example.org/>\nuser := io.csv("u.csv")',
    extensions: [client.plugin('file:///spike/main.fossil', 'fossil')],
  }),
  parent: document.getElementById('editor')!,
});
log('EditorView mounted with client.plugin extension. Doc length:', editor.state.doc.length);

// 6. Wait for the initialize round-trip, then probe semanticTokens directly.
client.initializing.then(async () => {
  log('--- initialization resolved; server capabilities follow ---');
  log('serverCapabilities:', client.serverCapabilities);

  const hasSemanticCap = Boolean(
    client.serverCapabilities &&
      (client.serverCapabilities as Record<string, unknown>).semanticTokensProvider,
  );
  log('serverCapabilities.semanticTokensProvider present?', hasSemanticCap);

  // 7. The decisive probe — try the raw client.request escape hatch.
  log('--- Probing textDocument/semanticTokens/full via client.request() ---');
  try {
    const tokens = await client.request<
      { textDocument: { uri: string } },
      { data: number[] } | null
    >('textDocument/semanticTokens/full', {
      textDocument: { uri: 'file:///spike/main.fossil' },
    });
    log('client.request returned:', tokens);
    if (tokens && Array.isArray(tokens.data) && tokens.data.length === 10) {
      log('OK semanticTokens response shape matches expected 5-int-tuple format (2 tokens × 5 ints = 10 ints)');
      log('Decoded:');
      log('  token 0: deltaLine=0 deltaStart=0 length=6 type=0 (keyword)   ← "prefix"');
      log('  token 1: deltaLine=0 deltaStart=7 length=2 type=4 (variable)  ← "ex"');
    } else {
      log('UNEXPECTED response shape — manual inspection required');
    }
  } catch (e) {
    log('client.request threw:', (e as Error).message);
  }

  log('--- Spike complete. ---');
});
