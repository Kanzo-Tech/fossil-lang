import { describe, it, expect, vi } from 'vitest';
import { NullTransport } from '../../src/transports/Null.js';

describe('NullTransport', () => {
  it('send() is a no-op (does not throw, does not invoke anything)', () => {
    const t = new NullTransport();
    expect(() =>
      t.send('{"jsonrpc":"2.0","id":1,"method":"x"}'),
    ).not.toThrow();
  });

  it('subscribe + send — handler is never called (no incoming messages)', () => {
    const t = new NullTransport();
    const handler = vi.fn();
    t.subscribe(handler);
    t.send('{}');
    expect(handler).not.toHaveBeenCalled();
  });

  it('unsubscribe() is a no-op + does not throw', () => {
    const t = new NullTransport();
    const handler = vi.fn();
    t.subscribe(handler);
    expect(() => t.unsubscribe(handler)).not.toThrow();
  });

  it('close() is a no-op + does not throw', () => {
    const t = new NullTransport();
    expect(() => t.close()).not.toThrow();
  });

  it('honors SendOptions.signal (ignored, no throw)', () => {
    const t = new NullTransport();
    const controller = new AbortController();
    expect(() =>
      t.send('{"jsonrpc":"2.0","id":1}', { signal: controller.signal }),
    ).not.toThrow();
  });
});
