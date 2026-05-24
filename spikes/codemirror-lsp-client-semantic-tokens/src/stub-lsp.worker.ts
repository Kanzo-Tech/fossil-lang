/// <reference lib="webworker" />
//
// Stub LSP server inside a Web Worker.
//
// Purpose: provide just enough of an LSP-conformant server for
// `@codemirror/lsp-client` to complete its initialize handshake and answer a
// `textDocument/semanticTokens/full` request. We do NOT pretend to be a real
// language server — only the messages the spike asks for are wired.
//
// The spike question this answers:
//   - Does the official @codemirror/lsp-client (v6.2.4, announced 2025-07-02)
//     surface LSP `textDocument/semanticTokens/full` responses in a usable way,
//     either via a built-in feature OR via the raw `client.request()` API?
//
// We return CANNED token data (5-int delta format) so the spike can verify
// the round-trip end-to-end.

declare const self: DedicatedWorkerGlobalScope;

interface JsonRpcRequest {
  jsonrpc: '2.0';
  id?: number | string;
  method: string;
  params?: unknown;
}

self.addEventListener('message', (e: MessageEvent) => {
  const raw = typeof e.data === 'string' ? e.data : JSON.stringify(e.data);
  let msg: JsonRpcRequest;
  try {
    msg = JSON.parse(raw) as JsonRpcRequest;
  } catch (err) {
    console.error('[WORKER] failed to parse message', raw, err);
    return;
  }

  // Log every incoming method — visible in DevTools when running locally.
  console.log('[WORKER] <-', msg.method, msg.id !== undefined ? `(id=${msg.id})` : '(notification)');

  if (msg.method === 'initialize') {
    respond(msg.id!, {
      capabilities: {
        textDocumentSync: 1, // Full
        semanticTokensProvider: {
          legend: {
            // 5 fake token types — matches the canned data below.
            tokenTypes: ['keyword', 'string', 'number', 'comment', 'variable'],
            tokenModifiers: [],
          },
          full: true,
          range: false,
        },
        // Minimal extra capabilities to keep the client quiet during init.
        completionProvider: { triggerCharacters: ['@'] },
        hoverProvider: true,
      },
      serverInfo: { name: 'fossil-spike-stub-lsp', version: '0.0.0' },
    });
  } else if (msg.method === 'initialized') {
    // notification — no response
  } else if (msg.method === 'textDocument/didOpen' || msg.method === 'textDocument/didChange' || msg.method === 'textDocument/didClose') {
    // notifications — no response
  } else if (msg.method === 'textDocument/semanticTokens/full') {
    // 5-int delta format: [deltaLine, deltaStart, length, tokenType, modifierMask]
    // Two tokens on line 0:
    //   - "prefix" (kind 0 = keyword) at offset 0, length 6
    //   - "ex"     (kind 4 = variable) at offset 7 (delta 7 after prev token), length 2
    respond(msg.id!, {
      data: [
        0, 0, 6, 0, 0, // "prefix"
        0, 7, 2, 4, 0, // "ex"
      ],
    });
  } else if (msg.method === 'shutdown') {
    respond(msg.id!, null);
  } else if (msg.method === 'exit') {
    self.close();
  } else if (msg.id !== undefined) {
    // Unknown REQUEST — return null result so the client doesn't hang on a timeout.
    console.warn('[WORKER] unknown request method', msg.method, '— responding with null result');
    respond(msg.id, null);
  }
});

function respond(id: number | string, result: unknown): void {
  const payload = JSON.stringify({ jsonrpc: '2.0', id, result });
  console.log('[WORKER] ->', payload.slice(0, 200) + (payload.length > 200 ? '…' : ''));
  self.postMessage(payload);
}
