import { describe, it, expect, vi, beforeEach } from 'vitest';
import { HttpTransport } from '../../src/transports/Http.js';

function mockFetchResponse(body: string, init: ResponseInit = {}): Response {
  return new Response(body, { status: 200, ...init });
}

describe('HttpTransport', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn();
  });

  it('POSTs the message body to the configured endpoint with content-type application/json', async () => {
    fetchMock.mockResolvedValue(mockFetchResponse(''));
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      fetch: fetchMock as unknown as typeof fetch,
    });
    t.send('{"jsonrpc":"2.0","id":1,"method":"hover"}');
    // Wait for fetch to resolve.
    await new Promise((r) => setTimeout(r, 0));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/fossil/analyze');
    expect(init.method).toBe('POST');
    expect(init.body).toBe('{"jsonrpc":"2.0","id":1,"method":"hover"}');
    const headers = new Headers(init.headers);
    expect(headers.get('content-type')).toBe('application/json');
  });

  it('forwards custom headers (auth) to fetch', async () => {
    fetchMock.mockResolvedValue(mockFetchResponse(''));
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      headers: { authorization: 'Bearer abc' },
      fetch: fetchMock as unknown as typeof fetch,
    });
    t.send('{}');
    await new Promise((r) => setTimeout(r, 0));
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    const headers = new Headers(init.headers);
    expect(headers.get('authorization')).toBe('Bearer abc');
  });

  it('dispatches the JSON-RPC response body to subscribed handlers', async () => {
    const responseBody = '{"jsonrpc":"2.0","id":1,"result":{"contents":"hello"}}';
    fetchMock.mockResolvedValue(mockFetchResponse(responseBody));
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      fetch: fetchMock as unknown as typeof fetch,
    });
    const handler = vi.fn();
    t.subscribe(handler);
    t.send('{"jsonrpc":"2.0","id":1,"method":"hover"}');
    await new Promise((r) => setTimeout(r, 5));
    expect(handler).toHaveBeenCalledWith(responseBody);
  });

  it('honors AbortSignal — passes it through to fetch', async () => {
    fetchMock.mockResolvedValue(mockFetchResponse(''));
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      fetch: fetchMock as unknown as typeof fetch,
    });
    const controller = new AbortController();
    t.send('{}', { signal: controller.signal });
    await new Promise((r) => setTimeout(r, 0));
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.signal).toBeInstanceOf(AbortSignal);
  });

  it('close() aborts in-flight requests + clears handlers', async () => {
    fetchMock.mockImplementation(
      (_url: string, init: RequestInit) =>
        new Promise<Response>((_, rej) => {
          init.signal?.addEventListener('abort', () =>
            rej(new DOMException('aborted', 'AbortError')),
          );
        }),
    );
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      fetch: fetchMock as unknown as typeof fetch,
    });
    const handler = vi.fn();
    t.subscribe(handler);
    t.send('{}');
    t.close();
    // Give the rejected promise a tick to propagate.
    await new Promise((r) => setTimeout(r, 5));
    expect(handler).not.toHaveBeenCalled();
  });

  it('does not dispatch on empty response body (notification responses are silent)', async () => {
    fetchMock.mockResolvedValue(mockFetchResponse(''));
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      fetch: fetchMock as unknown as typeof fetch,
    });
    const handler = vi.fn();
    t.subscribe(handler);
    t.send('{"jsonrpc":"2.0","method":"$/notification"}');
    await new Promise((r) => setTimeout(r, 5));
    expect(handler).not.toHaveBeenCalled();
  });

  it('post-close send() is a silent no-op', async () => {
    fetchMock.mockResolvedValue(mockFetchResponse('{}'));
    const t = new HttpTransport({
      endpoint: '/api/fossil/analyze',
      fetch: fetchMock as unknown as typeof fetch,
    });
    t.close();
    t.send('{"jsonrpc":"2.0","id":99}');
    await new Promise((r) => setTimeout(r, 5));
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
