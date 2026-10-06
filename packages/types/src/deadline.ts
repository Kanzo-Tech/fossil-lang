/**
 * **Every wait ends** — `/docs/design/failure`, G1. A wait on something outside the process races a
 * deadline set where the wait is made; work inside it has the caller's signal and nothing else.
 *
 * {@link within} is the one helper, and every `await host.…` and every module fetch goes through it —
 * the root `eslint.config.mjs` holds that.
 */

import { FossilError } from './error.js';

/** How long a {@link Host} promise may take — credentials, connections, a job's completion. */
export const HOST_MS = 30_000;
/** How long a `.wasm` module may take to fetch and instantiate. */
export const MODULE_MS = 60_000;

/**
 * `wait`, given at most `ms`. It is handed a signal that aborts when the deadline passes or the
 * caller's `signal` does, so a wait that can be cancelled is; one that cannot is abandoned, and its
 * late answer is ignored.
 *
 * A deadline that passes rejects with `silent(ms)` — coded by who did not answer, never a bare
 * timeout. The caller's abort rejects with its own reason, as an engine's does.
 *
 * The clock is `setTimeout`, not `AbortSignal.timeout`, so a test's fake timers drive it.
 */
export async function within<T>(
  ms: number,
  wait: (signal: AbortSignal) => Promise<T>,
  { signal, silent }: { signal?: AbortSignal | undefined; silent: (after: number) => Error },
): Promise<T> {
  signal?.throwIfAborted();
  const deadline = new AbortController();
  const timer = setTimeout(() => deadline.abort(silent(ms)), ms);
  const either = signal === undefined ? deadline.signal : AbortSignal.any([signal, deadline.signal]);
  try {
    return await until(wait(either), either);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * `answer`, or the signal's reason as soon as it aborts: the wait is abandoned, and the work it was
 * waiting on — a call into wasm that takes no signal — finishes unread.
 */
export function until<T>(answer: Promise<T>, signal: AbortSignal | undefined): Promise<T> {
  if (signal === undefined) return answer;
  return new Promise<T>((resolve, reject) => {
    const stop = (): void => reject(signal.reason);
    answer.then(resolve, reject).finally(() => signal.removeEventListener('abort', stop));
    if (signal.aborted) stop();
    else signal.addEventListener('abort', stop, { once: true });
  });
}

/**
 * Boot a wasm module through `init`, within {@link MODULE_MS}, and fail as a {@link FossilError}:
 * the module that would not compile is `internal/bug`, and one that could not be fetched — refused,
 * missing or silent — is `module/unreachable`, with `after` when the deadline ended the wait and the
 * browser's own error kept as the cause.
 *
 * It memoizes nothing: {@link loader} does.
 */
export async function boot<T>(module: string, init: () => Promise<T>): Promise<T> {
  let silent: DOMException | undefined;
  try {
    return await within(MODULE_MS, () => init(), {
      silent: (after) => (silent = new DOMException(`${module} did not load within ${after} ms`, 'TimeoutError')),
    });
  } catch (cause) {
    const name = cause instanceof Error ? cause.name : '';
    if (name === 'CompileError' || name === 'LinkError' || name === 'RuntimeError') {
      throw FossilError.of('internal/bug', { what: `${module} did not instantiate` }, {
        cause,
      });
    }
    const timedOut = silent !== undefined && cause === silent;
    throw FossilError.of(
      'module/unreachable',
      timedOut ? { locator: module, after: MODULE_MS } : { locator: module },
      { cause },
    );
  }
}

/**
 * The loader of a wasm-bindgen module: the first call boots it through {@link boot}, handing `wasm`
 * to the glue as its `module_or_path`. A boot that succeeded is kept; one that failed is forgotten,
 * so the next call tries again rather than answering a dropped download for the life of the page.
 */
export function loader<I>(
  module: string,
  init: (options?: { module_or_path: I }) => Promise<unknown>,
): (wasm?: I) => Promise<unknown> {
  let booted: Promise<unknown> | null = null;
  return (wasm) => {
    if (booted === null) {
      const pending = boot(module, () => init(wasm === undefined ? undefined : { module_or_path: wasm }));
      booted = pending;
      pending.catch(() => {
        if (booted === pending) booted = null;
      });
    }
    return booted;
  };
}
