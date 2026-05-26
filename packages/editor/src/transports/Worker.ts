/**
 * WorkerTransport — wraps a Web Worker (running the fossil-wasm LSP) into
 * the Transport interface. Extracted from packages/playground/src/lsp/
 * WorkerTransport.ts (v0.1 Phase 8); class form added for Phase 11 EDIT-02.
 *
 * Per ADR-0026 + ADR-0032 + ADR-0036 — the Worker is long-lived; callers
 * own lifecycle. close() removes the message listener + terminates the
 * Worker + clears handlers.
 */
import type { Transport, SendOptions } from './types.js';

export interface WorkerTransportOpts {
  /** A constructed Worker. Caller owns boot (e.g., posting `__boot` with
      wasmUrl per the fossil-wasm LSP protocol). Caller also owns
      termination unless close() is called on the transport. */
  worker: Worker;
}

export class WorkerTransport implements Transport {
  private worker: Worker;
  private handlers = new Set<(msg: string) => void>();
  private listener: (e: MessageEvent) => void;

  constructor(opts: WorkerTransportOpts) {
    this.worker = opts.worker;
    this.listener = (e: MessageEvent) => {
      // The Phase 7 07-03 dispatch loop posts STRINGS (the JSON-encoded
      // LspResponse). Defensive: if a future Worker variant posts objects
      // instead, JSON-encode them so handlers always see strings (per the
      // Transport contract).
      const msg = typeof e.data === 'string' ? e.data : JSON.stringify(e.data);
      this.handlers.forEach((h) => h(msg));
    };
    this.worker.addEventListener('message', this.listener);
  }

  send(msg: string, _opts?: SendOptions): void {
    // SendOptions.signal is intentionally ignored — Workers cannot cancel
    // in-flight postMessage. Documented in the Transport interface JSDoc
    // and ADR-0036.
    this.worker.postMessage(msg);
  }

  subscribe(handler: (msg: string) => void): void {
    this.handlers.add(handler);
  }

  unsubscribe(handler: (msg: string) => void): void {
    this.handlers.delete(handler);
  }

  close(): void {
    this.worker.removeEventListener('message', this.listener);
    this.worker.terminate();
    this.handlers.clear();
  }
}

/**
 * Functional form — backwards-compat for v0.1.x consumers (the playground's
 * useLspWorker.ts). New consumers should use `new WorkerTransport({ worker })`.
 *
 * The returned transport never owns the Worker's lifecycle by default —
 * callers are responsible for `worker.terminate()` (per ADR-0026, this
 * happens only on tab close for the LSP Worker; never on Reset).
 */
export function createWorkerTransport(worker: Worker): Transport {
  return new WorkerTransport({ worker });
}
