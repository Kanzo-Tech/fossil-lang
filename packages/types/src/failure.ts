/**
 * A failure as data, and back: what crosses a `postMessage` when a run is in a worker
 * (`/docs/design/off-the-page`). An `Error` loses its class in a structured clone, so each side
 * sends the failure as data and the other rebuilds it.
 *
 * - A {@link FossilError} crosses as its {@link Problem} and is rebuilt with `FossilError.from` —
 *   code, data, detail, related and causes, the same object the wasm crates build in Rust.
 * - Any other failure — a host's refusal, an `ApiError` coded `store/refused` — crosses as its
 *   `name` and `message`, and its own `code`, `title` and `data` when it has them, and is rebuilt as
 *   an `Error` carrying them as own properties: fossil reads a cause's `code` and `data` off whatever
 *   a host call rejected with.
 */

import { FossilError, isFossilError, type Problem } from './error.js';

/** A failure that can cross a structured clone. */
export type Failed =
  | { readonly problem: Problem }
  | {
      readonly name: string;
      readonly message: string;
      readonly code?: unknown;
      readonly title?: unknown;
      readonly data?: unknown;
    };

/** `e`, as data. */
export function failureOf(e: unknown): Failed {
  if (isFossilError(e)) return { problem: e.problem };
  if (typeof e !== 'object' || e === null) return { name: 'Error', message: String(e) };
  const { name, message, code, title, data } = e as Record<string, unknown>;
  const out: { -readonly [K in keyof Failed]?: unknown } & Record<string, unknown> = {
    name: typeof name === 'string' ? name : 'Error',
    message: typeof message === 'string' ? message : String(e),
  };
  if (code !== undefined) out.code = code;
  if (typeof title === 'string') out.title = title;
  if (data !== undefined) {
    try {
      out.data = JSON.parse(JSON.stringify(data));
    } catch {
      // A cycle or a BigInt: the code is kept without its data.
    }
  }
  return out as Failed;
}

/** The failure {@link failureOf} sent, rebuilt on this side. */
export function rebuilt(failed: Failed): Error {
  if ('problem' in failed) return FossilError.from(failed.problem);
  const error = new Error(failed.message) as Error & Record<string, unknown>;
  error.name = failed.name;
  for (const key of ['code', 'title', 'data'] as const) {
    if (failed[key] !== undefined) error[key] = failed[key];
  }
  return error;
}
