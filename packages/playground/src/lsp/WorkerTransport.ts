/**
 * Worker ↔ @codemirror/lsp-client Transport adapter.
 *
 * Per ADR-0032 (LSP-client choice) + the Wave-0 spike at
 * `spikes/codemirror-lsp-client-semantic-tokens/`. The `Transport` interface
 * (`{ send, subscribe, unsubscribe }`) is declared at the package's own
 * `dist/index.d.ts` line 170; this adapter wraps a Web Worker's
 * `postMessage` / `onmessage` channel into that shape.
 *
 * The fossil-wasm LSP Worker (Phase 7 07-03) speaks raw LSP JSON-RPC over
 * `postMessage`. The Worker side `start_lsp_worker()` (re-exported from
 * `crates/fossil-wasm/src/lsp_worker.rs`) installs an `onmessage` handler
 * that decodes the incoming string into `LspRequest`, dispatches it, and
 * posts the JSON-RPC response back as a string. This adapter is a pure
 * fan-out: it broadcasts every inbound message to every registered handler.
 *
 * Per RESEARCH.md Pattern 2 — a few-line adapter, library-agnostic. If
 * `@codemirror/lsp-client` is ever swapped for the documented fallback
 * (`codemirror-languageserver` per ADR-0032's Plan B), only the type import
 * changes.
 */

import type { Transport } from '@codemirror/lsp-client';

/**
 * Wrap a `Worker` into the `@codemirror/lsp-client` `Transport` shape.
 *
 * The returned transport never owns the Worker's lifecycle — callers are
 * responsible for `worker.terminate()` (which, per ADR-0026, happens only on
 * tab close for the LSP Worker; never on Reset).
 */
export function createWorkerTransport(worker: Worker): Transport {
  const handlers = new Set<(msg: string) => void>();

  worker.addEventListener('message', (e: MessageEvent) => {
    // The Phase 7 07-03 dispatch loop posts STRINGS (the JSON-encoded
    // LspResponse). Defensive: if a future Worker variant posts objects
    // instead, JSON-encode them so handlers always see strings (per the
    // Transport contract).
    const msg = typeof e.data === 'string' ? e.data : JSON.stringify(e.data);
    handlers.forEach((h) => h(msg));
  });

  return {
    send(msg: string) {
      worker.postMessage(msg);
    },
    subscribe(handler: (msg: string) => void) {
      handlers.add(handler);
    },
    unsubscribe(handler: (msg: string) => void) {
      handlers.delete(handler);
    },
  };
}
