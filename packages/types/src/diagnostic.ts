/**
 * A compile diagnostic as the wasm workspace returns it — `DiagnosticBase`, with `data` typed by
 * `code`. LSP's `Diagnostic`, and a problem like `Problem`: what went wrong, and where.
 */

import type { Problem } from './error.js';
import type { Code, ProblemData } from './problem.gen.js';
import type { DiagnosticBase } from './wire.gen.js';

export type { DiagnosticBase };

/** One diagnostic, discriminated by `code`: narrowing on the code narrows `data`. */
export type Diagnostic<C extends Code = Code> = {
  [K in C]: Omit<DiagnosticBase, 'code' | 'data'> & { code: K; data: ProblemData[K] };
}[C];

/**
 * A problem that has no place in the text, as the diagnostic a host counts: on the first character
 * of `uri`. Every diagnostic built from a {@link Problem} rather than by the workspace is this one,
 * so a failed check and an unreadable document are drawn alike.
 */
export function diagnosticOf(uri: string, problem: Problem, message: string = problem.detail): Diagnostic {
  return {
    uri,
    range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } },
    severity: 1,
    code: problem.code,
    data: problem.data,
    title: problem.title,
    message,
    ...(problem.help === undefined ? {} : { help: problem.help }),
  } as Diagnostic;
}
