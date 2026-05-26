/**
 * HttpTransport — POSTs JSON-RPC envelopes to a configurable endpoint.
 * Per Phase 11 CONTEXT.md + ADR-0036 (EDIT-02 transport mode #2; Keasy
 * backend integration).
 *
 * Request/response correlation: JSON-RPC `id` field. The transport tracks
 * pending requests in a Map<id, handler> equivalent — but since the CM6
 * Transport contract is `subscribe`-based (every incoming msg goes to
 * every handler), HttpTransport simply dispatches each fetch response to
 * the subscribed handlers verbatim (the CM6 LSPClient does correlation
 * upstream).
 *
 * Cancellation: SendOptions.signal is forwarded to the underlying
 * fetch(). On abort, the in-flight request is cancelled (Keasy backend
 * sees the connection drop); no response is dispatched.
 *
 * AbortSignal.any polyfill (Safari < 17.4): per ADR-0036, composeSignals
 * falls back to addEventListener-based composition when the native
 * AbortSignal.any is unavailable.
 *
 * Notifications (JSON-RPC requests with no `id`): same path — fire-and-
 * forget POST; the backend's response (if any) is dispatched to handlers.
 */
import type { Transport, SendOptions } from './types.js';

export interface HttpTransportOpts {
  /** Endpoint URL. Keasy uses '/api/fossil/analyze'. */
  endpoint: string;
  /** Optional fixed headers for every request (auth, content-type override). */
  headers?: HeadersInit;
  /** Optional fetch implementation. Defaults to global fetch. Useful for
      tests + Node environments. */
  fetch?: typeof fetch;
}

export class HttpTransport implements Transport {
  private endpoint: string;
  private headers: HeadersInit;
  private fetchImpl: typeof fetch;
  private handlers = new Set<(msg: string) => void>();
  private inFlight = new Set<AbortController>();
  private closed = false;

  constructor(opts: HttpTransportOpts) {
    this.endpoint = opts.endpoint;
    this.headers = opts.headers ?? {};
    this.fetchImpl = opts.fetch ?? globalThis.fetch.bind(globalThis);
  }

  send(msg: string, opts?: SendOptions): void {
    if (this.closed) return;
    const controller = new AbortController();
    // Compose signals: caller's signal + our own (for close()).
    const signal = opts?.signal
      ? composeSignals(opts.signal, controller.signal)
      : controller.signal;
    this.inFlight.add(controller);

    const headers = new Headers(this.headers);
    if (!headers.has('content-type'))
      headers.set('content-type', 'application/json');

    this.fetchImpl(this.endpoint, {
      method: 'POST',
      headers,
      body: msg,
      signal,
    })
      .then(async (res) => {
        const text = await res.text();
        // Defensive: only dispatch if we got a non-empty body and weren't aborted.
        if (text && !signal.aborted) {
          this.handlers.forEach((h) => h(text));
        }
      })
      .catch((err: unknown) => {
        // AbortError is expected on cancellation — swallow silently.
        // Other errors (network, 5xx propagated as throws by fetch wrapper)
        // are logged but not dispatched; the CM6 LSPClient times them out.
        const name = (err as { name?: string } | null)?.name;
        if (name !== 'AbortError') {
          // eslint-disable-next-line no-console
          console.warn('[HttpTransport] fetch failed:', err);
        }
      })
      .finally(() => {
        this.inFlight.delete(controller);
      });
  }

  subscribe(handler: (msg: string) => void): void {
    this.handlers.add(handler);
  }

  unsubscribe(handler: (msg: string) => void): void {
    this.handlers.delete(handler);
  }

  close(): void {
    this.closed = true;
    this.inFlight.forEach((c) => c.abort());
    this.inFlight.clear();
    this.handlers.clear();
  }
}

/**
 * AbortSignal composition — abort if either input signal aborts.
 * Polyfill of AbortSignal.any() for older runtimes (Safari < 17.4).
 * Per ADR-0036.
 */
function composeSignals(a: AbortSignal, b: AbortSignal): AbortSignal {
  // Native AbortSignal.any (Safari 17.4+, Chrome 116+, Firefox 124+). Call
  // through the class itself so the `this` binding is preserved across
  // runtime quirks (happy-dom in particular has a non-detachable any()
  // implementation; v8 engines accept either form).
  const Cls = AbortSignal as unknown as {
    any?: (signals: AbortSignal[]) => AbortSignal;
  };
  if (typeof Cls.any === 'function') {
    try {
      return Cls.any.call(AbortSignal, [a, b]);
    } catch {
      // Fall through to the polyfill below if the native any() rejects the
      // call (e.g., older happy-dom builds before v15.x).
    }
  }
  // Polyfill path — Safari < 17.4. Per ADR-0036.
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  a.addEventListener('abort', onAbort);
  b.addEventListener('abort', onAbort);
  if (a.aborted || b.aborted) controller.abort();
  return controller.signal;
}
