/**
 * The byte↔unit mapping, which is the one place a silent off-by-n lives.
 */
import { describe, expect, it } from 'vitest';

import { byteToUtf16Mapper } from '../src/offsets.js';

/** What the Rust lexer would report: a byte offset for each character start. */
function byteOffsets(text: string): number[] {
  const enc = new TextEncoder();
  const out: number[] = [];
  let byte = 0;
  for (const ch of text) {
    out.push(byte);
    byte += enc.encode(ch).length;
  }
  out.push(byte);
  return out;
}

/** Where CodeMirror would put each of those characters. */
function unitOffsets(text: string): number[] {
  const out: number[] = [];
  let unit = 0;
  for (const ch of text) {
    out.push(unit);
    unit += ch.length;
  }
  out.push(text.length);
  return out;
}

describe('byteToUtf16Mapper', () => {
  it('is the identity for ASCII, and allocates nothing to be', () => {
    const text = 'User := io.csv("users.csv")';
    const map = byteToUtf16Mapper(text);
    for (let i = 0; i <= text.length; i++) expect(map(i)).toBe(i);
  });

  it.each([
    ['two-byte', '// año de nacimiento\nUser := io.csv("u.csv")'],
    ['three-byte', 'name = "東京都"\nage = 3'],
    ['four-byte, a surrogate pair', 'label = "🪨 fossil"\nx = 1'],
    ['mixed, several times over', 'a"é"b"東"c"🪨"d — e'],
  ])('agrees with the encoder at every character start (%s)', (_name, text) => {
    const map = byteToUtf16Mapper(text);
    const bytes = byteOffsets(text);
    const units = unitOffsets(text);
    expect(bytes.length).toBe(units.length);
    for (let i = 0; i < bytes.length; i++) {
      expect(map(bytes[i]!)).toBe(units[i]!);
    }
  });

  it('never returns an offset outside the document', () => {
    const text = 'x = "🪨"';
    const map = byteToUtf16Mapper(text);
    for (let b = 0; b <= 64; b++) {
      const u = map(b);
      expect(u).toBeGreaterThanOrEqual(0);
      expect(u).toBeLessThanOrEqual(text.length);
    }
  });
});
