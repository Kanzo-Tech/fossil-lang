/**
 * CodeMirror document offset ↔ LSP `{ line, character }`.
 *
 * Everything fossil hands an editor is in UTF-16 code units, the units a JavaScript string is
 * indexed in, so this file is line-start arithmetic and nothing else.
 *
 * Both directions clamp. A position query is answered about the text as it was
 * when the request went out, and by the time the answer renders the user may
 * have deleted the line it points at — CodeMirror throws on an out-of-range
 * position, and an editor that crashes because you pressed backspace is worse
 * than a tooltip in the wrong place for one frame. Same argument as
 * `lint.ts`'s, which is why that module's clamp now lives here.
 */
import type { EditorState } from '@codemirror/state';
import type { Position, Range } from '@fossil-lang/types';

/** LSP position → document offset, clamped into the document. */
export function offsetOf(state: EditorState, pos: Position): number {
  const doc = state.doc;
  const lineNumber = Math.min(Math.max(pos.line + 1, 1), doc.lines);
  const line = doc.line(lineNumber);
  return Math.min(line.from + Math.max(pos.character, 0), line.to);
}

/** Document offset → LSP position. */
export function positionOf(state: EditorState, offset: number): Position {
  const clamped = Math.min(Math.max(offset, 0), state.doc.length);
  const line = state.doc.lineAt(clamped);
  return { line: line.number - 1, character: clamped - line.from };
}

/**
 * An LSP range as a CodeMirror `{ from, to }`, never zero-width.
 *
 * A zero-width range renders as nothing at all — no squiggle, no tooltip
 * target — so it is widened by one character where there is one. An error at
 * end-of-line is the case that needs it.
 */
export function rangeOf(state: EditorState, range: Range): { from: number; to: number } {
  const from = offsetOf(state, range.start);
  let to = offsetOf(state, range.end);
  if (to <= from) to = Math.min(from + 1, state.doc.length);
  return { from, to };
}
