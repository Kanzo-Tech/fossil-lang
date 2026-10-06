/**
 * A compile diagnostic as the wasm workspace returns it — `CheckRow` in
 * `crates/fossil-wasm/src/lib.rs`, the LSP diagnostic `fossil-ide` renders plus
 * the `uri` it is about. A problem, like `Problem`, and where it is.
 */

import type { Problem } from './error.js';
import type { Code, ProblemData } from './problem.gen.js';

/** Zero-based line + UTF-16 character column (LSP convention). */
export interface Position {
  line: number;
  character: number;
}

/** Inclusive `start`, exclusive `end` range (LSP convention). */
export interface Range {
  start: Position;
  end: Position;
}

/** One more place a diagnostic points at — LSP's `relatedInformation`. */
export interface CheckRelated {
  /** The key the file is open under, verbatim — the same string as `CheckRow.uri`. */
  uri: string;
  range: Range;
  message: string;
}

/** The fields every row has, whatever its code. */
export interface CheckRowBase {
  /** The key the buffer was opened under. */
  uri: string;
  range: Range;
  /** LSP `DiagnosticSeverity`: 1 = error, 2 = warning, 3 = information. */
  severity: 1 | 2 | 3;
  /** What a host branches on. */
  code: Code;
  /** Fixed per code. */
  title: string;
  /** The problem rendered, for a person; never parsed. */
  message: string;
  /** What to do about it, in prose. */
  help?: string;
  /** A replacement for a misspelt name: apply it as an edit. `range` is in the row's own file. */
  didYouMean?: { range: Range; replacement: string };
  /** Fossil source that repairs it, to replace {@link CheckRowBase.range} with. */
  suggestion?: string;
  related?: CheckRelated[];
}

/** One diagnostic, discriminated by `code`: narrowing on the code narrows `data`. */
export type CheckRow<C extends Code = Code> = {
  [K in C]: CheckRowBase & { code: K; data: ProblemData[K] };
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
