/**
 * The tag table, and the property that the deleted predecessor of this package
 * did not have.
 *
 * `tags.test.ts` used to assert `KIND_TO_TAG[FossilKind.KwPrefix] === 'keyword'`
 * against a hand-copied discriminant enum. That test passed for months while the
 * table was wrong in nine places, because the number it asserted and the number it
 * compared against were the same wrong number. These tests assert over NAMES, and
 * the one that would catch drift is in Rust — `token_kinds_legend_indexes_by_kind`
 * in `crates/fossil-wasm/tests/tokenize.rs` — because that is the side that knows.
 */
import { tags } from '@lezer/highlight';
import { describe, expect, it } from 'vitest';

import { TAG_BY_NAME, tagFor } from '../src/tags.js';

/** A legend shaped like the one `tokenKinds()` returns. */
const LEGEND = ['Whitespace', 'Newline', 'Comment', 'KwFrom', 'Ident', 'String'];

describe('tagFor', () => {
  it('reads the name out of the legend rather than the number', () => {
    expect(tagFor(LEGEND, 2)).toBe(tags.lineComment);
    expect(tagFor(LEGEND, 3)).toBe(tags.keyword);
    expect(tagFor(LEGEND, 5)).toBe(tags.string);
  });

  it('survives a reorder, which is the whole point', () => {
    // The same six names, shuffled — as a reorder of the Rust enum would produce.
    const reordered = ['String', 'KwFrom', 'Ident', 'Comment', 'Newline', 'Whitespace'];
    expect(tagFor(reordered, 0)).toBe(tags.string);
    expect(tagFor(reordered, 1)).toBe(tags.keyword);
    expect(tagFor(reordered, 3)).toBe(tags.lineComment);
  });

  it('gives identifiers no tag — the lexer cannot tell a type from a column', () => {
    expect(tagFor(LEGEND, 4)).toBeNull();
    expect(TAG_BY_NAME['Ident']).toBeUndefined();
  });

  it('gives whitespace no tag', () => {
    expect(tagFor(LEGEND, 0)).toBeNull();
    expect(tagFor(LEGEND, 1)).toBeNull();
  });

  it('treats a kind past the end of the legend as plain text, not a crash', () => {
    // A compiler newer than this host: it appended a variant we have never heard
    // of. The contract says that is backwards-compatible.
    expect(tagFor(LEGEND, 99)).toBeNull();
    expect(tagFor([], 0)).toBeNull();
  });

  it('treats a known kind with no mapping as plain text', () => {
    expect(tagFor(['SomethingNew'], 0)).toBeNull();
  });
});
