/// <reference lib="webworker" />
/**
 * LSP Worker entry — boots @fossil-lang/wasm and delegates LSP-over-postMessage
 * dispatch to the Rust-side `start_lsp_worker()`. Per ADR-0024 (`fossil-wasm` IS
 * the LSP server-side) + ADR-0026 (LSP Worker is long-lived, host-singleton).
 *
 * Implementation: `start_lsp_worker()` is exported by @fossil-lang/wasm
 * (re-exported from `crates/fossil-wasm/src/lsp_worker.rs:120`). It takes no
 * arguments, installs its own `self.onmessage` handler, and owns the full
 * 16-route LSP dispatch (initialize, textDocument/didOpen, didChange,
 * publishDiagnostics, semanticTokens/full, completion, hover, definition,
 * references, rename, formatting, codeAction, documentSymbol, signatureHelp,
 * shutdown, exit). The TypeScript side does NOT re-implement any of that.
 *
 * This Worker entry is a thin async boot wrapper: it awaits `initFossilWasm`
 * (consumer-controlled URL via the bundler's `?url` import), then calls
 * `start_lsp_worker()` exactly once. From that point on, the Worker is fully
 * driven by the Rust dispatcher — every postMessage flows through it.
 */

import { initFossilWasm, start_lsp_worker } from '@fossil-lang/wasm';

declare const self: DedicatedWorkerGlobalScope;

// Bootstrap message protocol: the main thread posts `{ type: '__boot', wasmUrl }`
// as its FIRST message. The Worker awaits init, calls `start_lsp_worker()`,
// then re-posts every queued message into the Rust dispatcher. This keeps the
// .wasm URL consumer-controlled (Pattern 3 / Pitfall 1) — the Worker entry has
// no hardcoded path.
const _preBoot: MessageEvent[] = [];
let _booted = false;

self.addEventListener('message', async (e: MessageEvent) => {
  if (_booted) {
    // start_lsp_worker has installed its own handler. This branch is only
    // reached for messages that arrived BEFORE the boot resolved — they're
    // drained from _preBoot below. After boot, the Rust dispatcher handles
    // everything via its own onmessage handler.
    return;
  }
  const data = e.data as { type?: string; wasmUrl?: string | URL };
  if (data && data.type === '__boot' && data.wasmUrl) {
    try {
      await initFossilWasm({ wasmUrl: data.wasmUrl });
      start_lsp_worker();
      _booted = true;
      // Replay any messages that arrived after __boot but before init resolved.
      for (const queued of _preBoot.splice(0)) {
        // Forward by re-dispatching — the Rust handler captures everything from now on.
        // `dispatchEvent` ensures the Rust onmessage handler picks it up.
        self.dispatchEvent(new MessageEvent('message', { data: queued.data }));
      }
    } catch (err) {
      // If boot fails, surface it back to the main thread. The LSPClient will
      // see the malformed payload and surface it as a connection error.
      self.postMessage(
        JSON.stringify({
          jsonrpc: '2.0',
          id: null,
          error: { code: -32603, message: `lsp.worker boot failed: ${String(err)}` },
        }),
      );
    }
  } else {
    // Non-boot message arrived before init — queue it. The Rust handler picks
    // up everything from `__boot` onward.
    _preBoot.push(e);
  }
});

export {};
