/** The tag tables: lexer names and semantic kinds → `@lezer/highlight` tags. */
import { tags } from '@lezer/highlight';
import { describe, expect, it } from 'vitest';

import { LEXICAL_KINDS, TAG_BY_NAME, semanticTagFor } from '../src/tags.js';

describe('TAG_BY_NAME', () => {
  it('gives identifiers no tag — the lexer cannot tell a type from a column', () => {
    expect(TAG_BY_NAME.Ident).toBeUndefined();
  });

  it('gives whitespace no tag', () => {
    expect(TAG_BY_NAME.Whitespace).toBeUndefined();
    expect(TAG_BY_NAME.Newline).toBeUndefined();
  });
});

describe('AtAttr', () => {
  it('is a special name, not a faint annotation', () => {
    expect(TAG_BY_NAME.AtAttr).toBe(tags.special(tags.variableName));
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
