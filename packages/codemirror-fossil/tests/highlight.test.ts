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

import type { SemanticTokenRow, TokenRow } from '@fossil-lang/types';

import { buildDecorations, type TokenSource } from '../src/highlight.js';

function source(rows: TokenRow[]): TokenSource {
  return { tokenize: () => rows };
}

/** A style that gives every tag we test a class we can recognise. */
const STYLE = HighlightStyle.define([
  { tag: tags.lineComment, class: 'tok-comment' },
  { tag: tags.keyword, class: 'tok-keyword' },
  { tag: tags.logicOperator, class: 'tok-logic' },
  { tag: tags.string, class: 'tok-string' },
  { tag: tags.namespace, class: 'tok-namespace' },
  { tag: tags.typeName, class: 'tok-type' },
  { tag: tags.variableName, class: 'tok-variable' },
  { tag: tags.definition(tags.variableName), class: 'tok-definition' },
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
        { kind: 'Comment', start: 0, end: 7 },
        { kind: 'KwFrom', start: 8, end: 12 },
        { kind: 'String', start: 13, end: 16 },
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
    expect(ranges(v, source([{ kind: 'KwFrom', start: 0, end: 4 }]))).toEqual([]);
    v.destroy();
  });

  it('skips whitespace and identifiers, which carry no tag', () => {
    const v = view('a b');
    expect(
      ranges(
        v,
        source([
          { kind: 'Ident', start: 0, end: 1 },
          { kind: 'Whitespace', start: 1, end: 2 },
          { kind: 'Ident', start: 2, end: 3 },
        ]),
      ),
    ).toEqual([]);
    v.destroy();
  });

  it('returns nothing rather than throwing when the module is not ready', () => {
    const v = view('from');
    const broken: TokenSource = {
      tokenize: () => {
        throw new Error('null pointer passed to rust');
      },
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
    };
    expect(ranges(v, counting, 2)).toEqual([]);
    expect(called).toBe(false);
    v.destroy();
  });

  it('drops a zero-width token rather than letting RangeSetBuilder throw', () => {
    const v = view('from');
    expect(ranges(v, source([{ kind: 'KwFrom', start: 2, end: 2 }]))).toEqual([]);
    v.destroy();
  });
});

describe('buildDecorations — the semantic layer', () => {
  /** One-line rows: `[from, to, kind, modifiers]` as UTF-16 columns on line 0. */
  function semantic(
    lexical: TokenRow[],
    rows: [number, number, string, string[]?][],
  ): TokenSource {
    return {
      tokenize: () => lexical,
      semanticTokens: (): SemanticTokenRow[] =>
        rows.map(([from, to, kind, modifiers = []]) => ({
          range: { start: { line: 0, character: from }, end: { line: 0, character: to } },
          kind,
          modifiers,
        })),
    };
  }

  it('paints the identifiers the lexer left plain', () => {
    // `users : Person`
    const v = view('users : Person');
    const got = ranges(
      v,
      semantic(
        [
          { kind: 'Ident', start: 0, end: 5 },
          { kind: 'Ident', start: 8, end: 14 },
        ],
        [
          [0, 5, 'variable', ['declaration']],
          [8, 14, 'type'],
        ],
      ),
    );
    expect(got).toEqual([
      [0, 5, 'tok-definition'],
      [8, 14, 'tok-type'],
    ]);
    v.destroy();
  });

  it('carves a connection out of the string around it', () => {
    // `"@lake/x.csv"` — the lexer sees one string, the compiler a connection in it.
    const doc = '"@lake/x.csv"';
    const v = view(doc);
    const got = ranges(
      v,
      semantic(
        [{ kind: 'String', start: 0, end: 13 }],
        [
          [0, 1, 'string'],
          [1, 6, 'namespace'],
          [6, 13, 'string'],
        ],
      ),
    );
    expect(got).toEqual([
      [0, 1, 'tok-string'],
      [1, 6, 'tok-namespace'],
      [6, 13, 'tok-string'],
    ]);
    expect(v.state.doc.sliceString(1, 6)).toBe('@lake');
    v.destroy();
  });

  it('lets a keyword fill a gap but not overwrite what the lexer named', () => {
    // `type a and b`: `type` is an Ident to the lexer, `and` already a logic operator.
    const v = view('type a and b');
    const got = ranges(
      v,
      semantic(
        [
          { kind: 'Ident', start: 0, end: 4 },
          { kind: 'KwAnd', start: 7, end: 10 },
        ],
        [
          [0, 4, 'keyword'],
          [7, 10, 'keyword'],
        ],
      ),
    );
    expect(got).toEqual([
      [0, 4, 'tok-keyword'],
      [7, 10, 'tok-logic'],
    ]);
    v.destroy();
  });

  it('keeps the lexical layer when the semantic call throws', () => {
    const v = view('from');
    const src: TokenSource = {
      tokenize: () => [{ kind: 'KwFrom', start: 0, end: 4 }],
      semanticTokens: () => {
        throw new Error('the workspace is busy');
      },
    };
    expect(ranges(v, src)).toEqual([[0, 4, 'tok-keyword']]);
    v.destroy();
  });

  it('leaves the lexical colour where the semantic kind has no tag', () => {
    const v = view('"x"');
    expect(
      ranges(v, semantic([{ kind: 'String', start: 0, end: 3 }], [[0, 3, 'string']])),
    ).toEqual([[0, 3, 'tok-string']]);
    v.destroy();
  });
});
