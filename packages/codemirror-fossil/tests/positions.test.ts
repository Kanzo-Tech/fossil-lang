/**
 * Offset ↔ LSP position, and the clamp.
 *
 * The round trip is the property worth holding: every offset in a document maps
 * to a position that maps back to it. A one-off in the line-start arithmetic
 * does not throw and does not look wrong — it puts a hover tooltip on the
 * character next door.
 */
import { EditorState } from '@codemirror/state';
import { describe, expect, it } from 'vitest';

import { offsetOf, positionOf, rangeOf } from '../src/positions.js';

const DOC = ['User := io.csv("users.csv")', '', 'People : Person from User', '    name = User.name'].join(
  '\n',
);

const state = (doc = DOC): EditorState => EditorState.create({ doc });

describe('positionOf / offsetOf', () => {
  it('round-trips every offset in the document', () => {
    const s = state();
    for (let offset = 0; offset <= s.doc.length; offset += 1) {
      expect(offsetOf(s, positionOf(s, offset))).toBe(offset);
    }
  });

  it('counts a non-ASCII character as its UTF-16 width, not its byte width', () => {
    // `// café` is seven UTF-16 units and EIGHT bytes — `é` is one unit and two
    // bytes. The lexer's offsets are bytes and need `byteToUtf16Mapper`; LSP
    // positions are units and must not, so the end of that line is character 7
    // and the next line starts at offset 8.
    const s = state('// café\nname = x');
    expect(positionOf(s, 7)).toEqual({ line: 0, character: 7 });
    expect(offsetOf(s, { line: 1, character: 0 })).toBe(8);
  });

  it('clamps a position past the end of the document', () => {
    const s = state();
    expect(offsetOf(s, { line: 999, character: 999 })).toBe(s.doc.length);
    expect(offsetOf(s, { line: -3, character: -3 })).toBe(0);
  });

  it('clamps a character past the end of its line to the line end', () => {
    const s = state();
    const line = s.doc.line(1);
    expect(offsetOf(s, { line: 0, character: 500 })).toBe(line.to);
  });
});

describe('rangeOf', () => {
  it('widens a zero-width range so it renders', () => {
    const s = state();
    const at = { line: 0, character: 4 };
    const { from, to } = rangeOf(s, { start: at, end: at });
    expect(to).toBe(from + 1);
  });

  it('cannot widen past the end of an empty document', () => {
    const s = state('');
    const at = { line: 0, character: 0 };
    expect(rangeOf(s, { start: at, end: at })).toEqual({ from: 0, to: 0 });
  });
});
