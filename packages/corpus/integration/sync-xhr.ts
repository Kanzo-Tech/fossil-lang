import { MessageChannel, Worker, receiveMessageOnPort } from 'node:worker_threads';

/**
 * A synchronous `XMLHttpRequest` for Node, which has none — the one thing DuckDB-WASM's httpfs asks
 * of its host to read `http://`. In a browser the page provides it; the blocking node bindings
 * call `new XMLHttpRequest()` with `async = false` and give up when the global is missing.
 *
 * `send` hands the request to a worker that `fetch`es it, and waits on an `Atomics` flag until the
 * answer is on the port. Only what the httpfs glue touches is implemented: `open`,
 * `setRequestHeader`, `responseType = 'arraybuffer'`, `send`, `status`, `response`,
 * `getResponseHeader` and `getAllResponseHeaders`.
 */
export function installSyncXhr(): () => Promise<number> {
  const { port1, port2 } = new MessageChannel();
  const worker = new Worker(
    `
    const { workerData: { port } } = require('node:worker_threads');
    port.on('message', async ({ method, url, headers, flag }) => {
      let answer;
      try {
        const res = await fetch(url, { method, headers });
        const body = method === 'HEAD' ? new ArrayBuffer(0) : await res.arrayBuffer();
        answer = { status: res.status, headers: [...res.headers], body };
      } catch (e) {
        answer = { status: 0, headers: [], body: new ArrayBuffer(0), error: String(e) };
      }
      port.postMessage(answer, [answer.body]);
      Atomics.store(flag, 0, 1);
      Atomics.notify(flag, 0);
    });
    `,
    { eval: true, workerData: { port: port2 }, transferList: [port2] },
  );
  worker.unref();

  class SyncXMLHttpRequest {
    status = 0;
    response: ArrayBuffer = new ArrayBuffer(0);
    responseType = '';
    timeout = 0;
    #method = 'GET';
    #url = '';
    #headers: Record<string, string> = {};
    #received: [string, string][] = [];

    open(method: string, url: string, async = true): void {
      if (async) throw new Error('this XMLHttpRequest is synchronous only');
      this.#method = method;
      this.#url = url;
    }
    setRequestHeader(name: string, value: string): void {
      this.#headers[name] = value;
    }
    overrideMimeType(): void {}
    send(): void {
      const flag = new Int32Array(new SharedArrayBuffer(4));
      port1.postMessage({ method: this.#method, url: this.#url, headers: this.#headers, flag });
      Atomics.wait(flag, 0, 0);
      const answer = receiveMessageOnPort(port1)!.message as {
        status: number;
        headers: [string, string][];
        body: ArrayBuffer;
        error?: string;
      };
      if (answer.error !== undefined) throw new Error(answer.error);
      this.status = answer.status;
      this.response = answer.body;
      this.#received = answer.headers;
    }
    get responseText(): string {
      return new TextDecoder().decode(this.response);
    }
    getResponseHeader(name: string): string | null {
      return this.#received.find(([k]) => k.toLowerCase() === name.toLowerCase())?.[1] ?? null;
    }
    getAllResponseHeaders(): string {
      return this.#received.map(([k, v]) => `${k}: ${v}\r\n`).join('');
    }
  }

  (globalThis as { XMLHttpRequest?: unknown }).XMLHttpRequest = SyncXMLHttpRequest;
  return () => {
    delete (globalThis as { XMLHttpRequest?: unknown }).XMLHttpRequest;
    port1.close();
    return worker.terminate();
  };
}
