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

import { LEXICAL_KINDS, TAG_BY_NAME, semanticTagFor, tagFor } from '../src/tags.js';

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

describe('AtAttr', () => {
  it('is a special name, not a faint annotation', () => {
    expect(TAG_BY_NAME['AtAttr']).toBe(tags.special(tags.variableName));
  });
});

describe('semanticTagFor', () => {
  it('maps each kind the compiler names to its tag', () => {
    expect(semanticTagFor('type', [])).toBe(tags.typeName);
    expect(semanticTagFor('function', [])).toBe(tags.function(tags.variableName));
    expect(semanticTagFor('property', [])).toBe(tags.propertyName);
    expect(semanticTagFor('parameter', [])).toBe(tags.attributeName);
    expect(semanticTagFor('variable', [])).toBe(tags.variableName);
    expect(semanticTagFor('namespace', [])).toBe(tags.namespace);
    expect(semanticTagFor('keyword', [])).toBe(tags.keyword);
  });

  it('wraps a declaration in tags.definition', () => {
    expect(semanticTagFor('variable', ['declaration'])).toBe(tags.definition(tags.variableName));
    expect(semanticTagFor('type', ['declaration'])).toBe(tags.definition(tags.typeName));
  });

  it('leaves what the lexer names finer to the lexer', () => {
    for (const kind of ['string', 'number', 'operator', 'comment', 'somethingNew']) {
      expect(semanticTagFor(kind, [])).toBeNull();
    }
    expect(LEXICAL_KINDS.has('keyword')).toBe(true);
    expect(LEXICAL_KINDS.has('type')).toBe(false);
  });
});
