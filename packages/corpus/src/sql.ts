/** The one place a value becomes SQL text — a literal, or an identifier — and reaches the engine. */

import { FossilError, type Engine } from '@fossil-lang/types';

type Answer = Awaited<ReturnType<Engine['query']>>;

/**
 * Run `statement` on the host's engine. An engine that refuses is `engine/failed`, its own error kept
 * whole as the cause; an abort rejects with the signal's reason, as it always has.
 */
export async function query(engine: Engine, statement: string, signal?: AbortSignal): Promise<Answer> {
  try {
    return await (signal === undefined ? engine.query(statement) : engine.query(statement, { signal }));
  } catch (cause) {
    signal?.throwIfAborted();
    throw FossilError.of('engine/failed', {}, 'the query engine failed', { cause });
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
