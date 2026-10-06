/**
 * A compile diagnostic as the wasm workspace returns it — `CheckRowBase`, with `data` typed by
 * `code`. A problem, like `Problem`, and where it is.
 */

import type { Problem } from './error.js';
import type { Code, ProblemData } from './problem.gen.js';
import type { CheckRowBase } from './wire.gen.js';

/** One diagnostic, discriminated by `code`: narrowing on the code narrows `data`. */
export type CheckRow<C extends Code = Code> = {
  [K in C]: Omit<CheckRowBase, 'code' | 'data'> & { code: K; data: ProblemData[K] };
}[C];

/**
 * A problem that has no place in the text, as the row a host counts: on the first character of
 * `uri`. Every row built from a {@link Problem} rather than by the workspace is this one, so a
 * failed check and an unreadable document are drawn alike.
 */
export function rowOf(uri: string, problem: Problem, message: string = problem.detail): CheckRow {
  return {
    uri,
    range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } },
    severity: 1,
    code: problem.code,
    data: problem.data,
    title: problem.title,
    message,
    ...(problem.help === undefined ? {} : { help: problem.help }),
  } as CheckRow;
}
