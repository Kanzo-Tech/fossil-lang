/**
 * NullTransport — no-op Transport. Use for read-only static editors
 * (no LSP wiring; syntactic highlighting + @-autocomplete still fire
 * via @fossil-lang/codemirror-fossil's `fossil()` extension).
 *
 * Per Phase 11 CONTEXT.md — one of the three locked transport modes.
 */
import type { Transport, SendOptions } from './types.js';

export class NullTransport implements Transport {
  send(_msg: string, _opts?: SendOptions): void {
    // No-op. The editor still renders; only LSP-driven features are inert.
  }

  subscribe(_handler: (msg: string) => void): void {
    // No-op. No server to receive messages from.
  }

  unsubscribe(_handler: (msg: string) => void): void {
    // No-op.
  }

  close(): void {
    // No-op.
  }
}
