/**
 * Transport interface + JSON-RPC envelope types.
 *
 * Per ADR-0036 (Transport-superset): this interface is a STRICT SUPERSET of
 * `@codemirror/lsp-client@^6.2.4`'s `Transport` shape (`send` / `subscribe` /
 * `unsubscribe` accepting stringified JSON-RPC envelopes). The superset adds:
 *   - SendOptions.signal (AbortSignal — HttpTransport honors; WorkerTransport
 *     ignores, documented)
 *   - close?() lifecycle hook (HttpTransport aborts in-flight; WorkerTransport
 *     terminates the Worker; NullTransport no-op)
 *
 * Structural typing: a Fossil Transport instance ALSO satisfies CM6's narrower
 * Transport interface (we only ADDED methods/optional params). The editor's
 * `buildLspExtension(transport)` passes our Transport directly to
 * `new LSPClient(...).connect(transport)`; TypeScript narrowing happens at
 * the boundary.
 */

/** JSON-RPC 2.0 envelope (LSP wire format). */
export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: unknown;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: number | string;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

export interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

export interface SendOptions {
  /** Caller-supplied AbortSignal — HttpTransport aborts the in-flight fetch;
      WorkerTransport ignores (Workers cannot cancel in-flight messages). */
  signal?: AbortSignal;
}

/**
 * Pluggable LSP transport. Subsumes @codemirror/lsp-client's send/subscribe/
 * unsubscribe pattern AND extends it with cancellation + lifecycle.
 *
 * IMPL CONTRACT:
 *  - send(msg): MUST accept a stringified JSON-RPC envelope (matches CM6 contract)
 *  - subscribe/unsubscribe: handlers receive stringified JSON-RPC envelopes from server
 *  - close(): release resources (terminate Worker; abort in-flight HTTP)
 *  - SendOptions.signal: HttpTransport honors; WorkerTransport ignores (documented)
 */
export interface Transport {
  send(msg: string, opts?: SendOptions): void;
  subscribe(handler: (msg: string) => void): void;
  unsubscribe(handler: (msg: string) => void): void;
  close?(): void;
}
