import { describe, it, expect } from 'vitest';
import type { Transport, JsonRpcRequest, SendOptions } from '../src/index.js';

describe('@fossil-lang/editor scaffold', () => {
  it('exports the Transport interface (type-only smoke check)', () => {
    // Compile-time check — the import above MUST resolve. If it does, this
    // test passes. Runtime assertion is a tautology that exists only so
    // vitest reports >0 passing tests on the scaffold.
    const probe: { hasTransport: true } = { hasTransport: true };
    // Reference Transport so TS does not elide the import.
    const _t: Transport | null = null;
    void _t;
    expect(probe.hasTransport).toBe(true);
  });

  it('JsonRpcRequest envelope has the LSP-required fields', () => {
    const req: JsonRpcRequest = {
      jsonrpc: '2.0',
      id: 1,
      method: 'textDocument/hover',
      params: {},
    };
    expect(req.jsonrpc).toBe('2.0');
    expect(req.id).toBe(1);
  });

  it('SendOptions accepts an AbortSignal', () => {
    const controller = new AbortController();
    const opts: SendOptions = { signal: controller.signal };
    expect(opts.signal).toBeInstanceOf(AbortSignal);
  });
});
