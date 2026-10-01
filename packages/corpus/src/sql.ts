/** The one place a value becomes SQL text — a literal, or an identifier — and reaches the engine. */

import { FossilError, type Engine } from '@fossil-lang/types';

type Answer = Awaited<ReturnType<Engine['query']>>;

/**
 * Run `statement` on the host's engine. An engine that refuses is `engine/failed`, its own error kept
 * whole as the cause; an abort rejects with the signal's reason, as it always has. Without a signal
 * the statement carries one nothing aborts — a cleanup the caller's Stop must not interrupt.
 */
export async function query(engine: Engine, statement: string, signal?: AbortSignal): Promise<Answer> {
  try {
    return await engine.query(statement, { signal: signal ?? new AbortController().signal });
  } catch (cause) {
    signal?.throwIfAborted();
    throw FossilError.of('engine/failed', {}, { cause });
  }
}

/** A single-quoted SQL string literal. Every path the reader composes reaches SQL through here. */
export function lit(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

/** A quoted SQL identifier. Names come from the manifest, so they are never interpolated raw. */
export function ident(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}
