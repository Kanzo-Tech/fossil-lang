/**
 * The run's worker and the page, as messages — both halves, in the one module both sides import
 * (`/docs/design/off-the-page`).
 *
 * The worker holds the executor; the page keeps the two things only it has, the `Host` and the
 * `signal`:
 *
 * | from   | message            | what it carries                                                 |
 * | ------ | ------------------ | --------------------------------------------------------------- |
 * | worker | `ready`            | it started: a death after this is a crash, not a missing module |
 * | page   | `run`              | the compiled module, the program and the run's options — `files` copied |
 * | worker | `ask`              | `connections`, or `credentials(scope, access)`, with an id      |
 * | page   | `answer`/`refuse`  | the host's answer to that id, or its failure as data            |
 * | worker | `done`             | the run's answer, its files' buffers transferred, each once     |
 * | worker | `failed`           | the run's failure, as data                                      |
 *
 * A failure crosses as data and is rebuilt on the far side (`@fossil-lang/types/internal`'s
 * `failureOf`): a structured clone keeps no class, and fossil reads a cause's `code` and `data` off
 * whatever a host call rejected with.
 */

import { FossilError, HOST_MS, type Access, type Host, type Profiled, type Scope } from '@fossil-lang/types';
import { failureOf, rebuilt, within, type Failed } from '@fossil-lang/types/internal';

/** What both ends are: a `Worker` on the page, the worker's own scope inside it, a `MessagePort`. */
export interface Port {
  postMessage(message: unknown, transfer?: Transferable[]): void;
  onmessage: ((event: MessageEvent) => unknown) | null;
}

/** The run the page asks of the worker. */
export interface Request {
  readonly type: 'run';
  /** The executor's module, compiled on the page; absent where the worker boots it itself. */
  readonly module?: WebAssembly.Module;
  readonly program: string;
  readonly into: { job: string } | 'memory';
  readonly limit?: number;
  readonly profiled?: Profiled;
  /** Whether the run reads through the page's host, which the worker then asks. */
  readonly host: boolean;
  readonly files?: Readonly<Record<string, Uint8Array>>;
  readonly path?: string;
}

type ToWorker =
  | Request
  | { readonly type: 'answer'; readonly id: number; readonly value: unknown }
  | { readonly type: 'refuse'; readonly id: number; readonly failure: Failed };

type Ask =
  | { readonly call: 'connections' }
  | { readonly call: 'credentials'; readonly scope: Scope; readonly access: Access };

type ToPage =
  | { readonly type: 'ready' }
  | ({ readonly type: 'ask'; readonly id: number } & Ask)
  | { readonly type: 'done'; readonly result: unknown }
  | { readonly type: 'failed'; readonly failure: Failed };

/** What the page's half is handed beside the port and the request. */
export interface Drive {
  readonly host?: Host;
  readonly signal?: AbortSignal;
  /** Ends the worker — on an abort, and once the run has answered. */
  terminate(): void;
  /** Subscribes to the worker dying — a script that would not load, a trap, an uncaught error. */
  crashes?(listener: (message: string) => void): void;
}

/**
 * The page's half: post `request`, answer the worker's host calls with `host`, and settle with the
 * run's answer or its failure, rebuilt.
 *
 * **The signal terminates the worker.** An abort calls `terminate`, rejects with `signal.reason` and
 * aborts the host calls still in flight; termination does not wait for DataFusion to reach an
 * `await`. A worker that dies before it said `ready` rejects as `module/unreachable`, and one that
 * dies after as `internal/bug`.
 */
export function drive(port: Port, request: Request, { host, signal, terminate, crashes }: Drive): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const calls = new Set<AbortController>();
    let started = false;
    let settled = false;
    const settle = (finish: () => void): void => {
      if (settled) return;
      settled = true;
      for (const call of calls) call.abort();
      calls.clear();
      port.onmessage = null;
      signal?.removeEventListener('abort', aborted);
      terminate();
      finish();
    };
    const aborted = (): void => settle(() => reject(signal!.reason));
    if (signal?.aborted) {
      aborted();
      return;
    }
    signal?.addEventListener('abort', aborted, { once: true });
    crashes?.((message) =>
      settle(() =>
        reject(
          started
            ? FossilError.of('internal/bug', { what: `the run's worker died: ${message}` })
            : FossilError.of('module/unreachable', { location: 'the executor worker' }, { cause: new Error(message) }),
        ),
      ),
    );

    port.onmessage = async ({ data }) => {
      const message = data as ToPage;
      switch (message.type) {
        case 'ready':
          started = true;
          return;
        case 'ask': {
          // A host call is the host's call, with a signal of the page's own, aborted when the run
          // ends however it ends: a credential still in flight is cancelled, not left to resolve.
          const call = new AbortController();
          calls.add(call);
          try {
            if (host === undefined) throw FossilError.of('internal/bug', { what: 'a run over files asked for a host' });
            // Held to the deadline every host wait is held to, as the run in this thread holds it.
            const scope =
              message.call === 'connections' ? 'the connections' : JSON.stringify(message.scope);
            const value = await within<unknown>(
              HOST_MS,
              (bounded): Promise<unknown> =>
                message.call === 'connections'
                  ? host.connections({ signal: bounded })
                  : host.credentials(message.scope, message.access, { signal: bounded }),
              { signal: call.signal, silent: (after) => FossilError.of('storage/host-silent', { scope, after }) },
            );
            if (!settled) port.postMessage({ type: 'answer', id: message.id, value } satisfies ToWorker);
          } catch (e) {
            if (!settled) port.postMessage({ type: 'refuse', id: message.id, failure: failureOf(e) } satisfies ToWorker);
          } finally {
            calls.delete(call);
          }
          return;
        }
        case 'done':
          settle(() => resolve(message.result));
          return;
        case 'failed':
          settle(() => reject(rebuilt(message.failure)));
          return;
      }
    };
    port.postMessage(request, []);
  });
}

/** What the worker's half runs: the request, with the page's host when it reads through one. */
export type Runner = (request: Request, host: Host | undefined) => Promise<unknown>;

/**
 * The worker's half: say `ready`, run the request with a `Host` that asks the page, and answer —
 * every `Uint8Array` buffer of the answer transferred, each once — or fail as data.
 */
export function serve(port: Port, runner: Runner): void {
  let next = 0;
  const waiting = new Map<number, { resolve(value: unknown): void; reject(error: unknown): void }>();
  const ask = <T>(question: Ask): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      const id = next++;
      waiting.set(id, { resolve: resolve as (value: unknown) => void, reject });
      port.postMessage({ type: 'ask', id, ...question } satisfies ToPage);
    });
  const host: Host = {
    connections: () => ask({ call: 'connections' }),
    credentials: (scope, access) => ask({ call: 'credentials', scope, access }),
  };

  port.onmessage = ({ data }) => {
    const message = data as ToWorker;
    switch (message.type) {
      case 'run':
        runner(message, message.host ? host : undefined).then(
          (result) => port.postMessage({ type: 'done', result } satisfies ToPage, buffersOf(result)),
          (e: unknown) => port.postMessage({ type: 'failed', failure: failureOf(e) } satisfies ToPage),
        );
        return;
      case 'answer':
      case 'refuse': {
        const pending = waiting.get(message.id);
        waiting.delete(message.id);
        if (message.type === 'answer') pending?.resolve(message.value);
        else pending?.reject(rebuilt(message.failure));
        return;
      }
    }
  };
  port.postMessage({ type: 'ready' } satisfies ToPage);
}

/** The buffers of an answer's files, each once: a buffer transferred twice is a `DataCloneError`. */
function buffersOf(result: unknown): Transferable[] {
  const files = (result as { files?: unknown } | null)?.files;
  if (!Array.isArray(files)) return [];
  const buffers = new Set<ArrayBuffer>();
  for (const file of files) {
    const bytes = (file as { bytes?: unknown }).bytes;
    if (bytes instanceof Uint8Array && bytes.buffer instanceof ArrayBuffer) buffers.add(bytes.buffer);
  }
  return [...buffers];
}
