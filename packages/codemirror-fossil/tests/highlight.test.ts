/**
 * The decoration pass, driven against a fake token source.
 *
 * No wasm here on purpose: `TokenSource` is injected, so the mapping from rows to
 * decorations is testable without instantiating 3.5 MB of compiler. What the fake
 * cannot check — that the rows are the rows the real lexer emits — is checked on
 * the Rust side, where the lexer is.
 */
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { tags } from '@lezer/highlight';
import { describe, expect, it } from 'vitest';

import { buildDecorations, type TokenSource } from '../src/highlight.js';

/** A legend in the shape `tokenKinds()` returns, and rows in `tokenize()`'s. */
const LEGEND = ['Whitespace', 'Comment', 'KwFrom', 'Ident', 'String'];

function source(rows: { kind: number; start: number; end: number }[]): TokenSource {
  return { tokenize: () => rows, tokenKinds: () => LEGEND };
}

/** A style that gives every tag we test a class we can recognise. */
const STYLE = HighlightStyle.define([
  { tag: tags.lineComment, class: 'tok-comment' },
  { tag: tags.keyword, class: 'tok-keyword' },
  { tag: tags.string, class: 'tok-string' },
]);

function view(doc: string, extensions = [syntaxHighlighting(STYLE)]): EditorView {
  return new EditorView({ state: EditorState.create({ doc, extensions }) });
}

/** Flatten a DecorationSet into `[from, to, class]` triples. */
function ranges(v: EditorView, src: TokenSource, max = 200_000): [number, number, string][] {
  const set = buildDecorations(v, src, max);
  const out: [number, number, string][] = [];
  const iter = set.iter();
  while (iter.value) {
    out.push([iter.from, iter.to, (iter.value.spec as { class: string }).class]);
    iter.next();
  }
  return out;
}

describe('buildDecorations', () => {
  it('marks each token with the class the active highlight style gives its tag', () => {
    const doc = '// note\nfrom "x"';
    const v = view(doc);
    const got = ranges(
      v,
      source([
        { kind: 1, start: 0, end: 7 }, // Comment
        { kind: 2, start: 8, end: 12 }, // KwFrom
        { kind: 4, start: 13, end: 16 }, // String
      ]),
    );
    expect(got).toEqual([
      [0, 7, 'tok-comment'],
      [8, 12, 'tok-keyword'],
      [13, 16, 'tok-string'],
    ]);
    v.destroy();
  });

  it('emits nothing when no highlight style is installed', () => {
    // `highlightingFor` returns null with no style in the facet. Painting a
    // fallback palette here would fight whatever theme the host actually has.
    const v = view('from "x"', []);
    expect(ranges(v, source([{ kind: 2, start: 0, end: 4 }]))).toEqual([]);
    v.destroy();
  });

  it('skips whitespace and identifiers, which carry no tag', () => {
    const v = view('a b');
    expect(
      ranges(
        v,
        source([
          { kind: 3, start: 0, end: 1 }, // Ident
          { kind: 0, start: 1, end: 2 }, // Whitespace
          { kind: 3, start: 2, end: 3 }, // Ident
        ]),
      ),
    ).toEqual([]);
    v.destroy();
  });

  it('shifts every offset past a multi-byte character', () => {
    // "// añ" is 5 characters and 6 bytes; the token after it starts at byte 6 and
    // at code unit 5. Getting this wrong colours the wrong text and throws nothing.
    const doc = '// añ\nfrom';
    const v = view(doc);
    const got = ranges(
      v,
      source([
        { kind: 1, start: 0, end: 6 }, // Comment, bytes
        { kind: 2, start: 7, end: 11 }, // KwFrom, bytes
      ]),
    );
    expect(got).toEqual([
      [0, 5, 'tok-comment'],
      [6, 10, 'tok-keyword'],
    ]);
    expect(v.state.doc.sliceString(6, 10)).toBe('from');
    v.destroy();
  });

  it('returns nothing rather than throwing when the module is not ready', () => {
    const v = view('from');
    const broken: TokenSource = {
      tokenize: () => {
        throw new Error('null pointer passed to rust');
      },
      tokenKinds: () => LEGEND,
    };
    expect(ranges(v, broken)).toEqual([]);
    v.destroy();
  });

  it('does not tokenize a document past maxLength', () => {
    let called = false;
    const v = view('from');
    const counting: TokenSource = {
      tokenize: () => {
        called = true;
        return [];
      },
      tokenKinds: () => LEGEND,
    };
    expect(ranges(v, counting, 2)).toEqual([]);
    expect(called).toBe(false);
    v.destroy();
  });

  it('drops a zero-width token rather than letting RangeSetBuilder throw', () => {
    const v = view('from');
    expect(ranges(v, source([{ kind: 2, start: 2, end: 2 }]))).toEqual([]);
    v.destroy();
  });
});
