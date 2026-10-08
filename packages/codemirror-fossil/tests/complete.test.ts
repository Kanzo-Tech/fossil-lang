/**
 * The completion half: the kind table, and where a replacement starts.
 *
 * The `from` arithmetic is the one that is not obvious. The compiler narrowed by
 * the receiver, so what comes back are MEMBERS — `trim`, not `str.trim` — and
 * inserting one at the start of `str.tr` would produce `trimstr.tr`. The tests
 * below pin the dot.
 */
import { EditorState } from '@codemirror/state';
import { CompletionContext } from '@codemirror/autocomplete';
import type { CompletionItem } from '@fossil-lang/types';
import { describe, expect, it } from 'vitest';

import { fossilCompletionSource, toCompletion } from '../src/complete.js';

const rows: CompletionItem[] = [
  { label: 'trim', kind: 'function', detail: 'str.trim(String) -> String', insertText: 'trim' },
  { label: 'upper', kind: 'function', detail: '', insertText: 'upper' },
];

function contextAt(doc: string, pos: number, explicit = false): CompletionContext {
  return new CompletionContext(EditorState.create({ doc }), pos, explicit);
}

describe('toCompletion', () => {
  it('maps the LSP kind name onto CodeMirror’s vocabulary', () => {
    expect(toCompletion({ label: 'x', kind: 'function', detail: '', insertText: 'x' }).type).toBe('function');
    // LSP has `field` and CodeMirror does not: it is CodeMirror's `property`.
    expect(toCompletion({ label: 'x', kind: 'field', detail: '', insertText: 'x' }).type).toBe('property');
  });

  it('leaves the type off for an item with no kind', () => {
    expect(toCompletion({ label: 'x', detail: '', insertText: 'x' }).type).toBeUndefined();
  });

  it('omits an empty detail instead of rendering a blank line', () => {
    expect(toCompletion({ label: 'x', kind: 'function', detail: '', insertText: 'x' }).detail).toBeUndefined();
    expect(toCompletion({ label: 'x', kind: 'function', detail: 'sig', insertText: 'x' }).detail).toBe('sig');
  });
});

describe('a quoted member', () => {
  const quoted: CompletionItem = {
    label: 'Person.id',
    kind: 'field',
    detail: 'source field : String',
    insertText: '"Person.id"',
  };

  it('inserts the spelling and lists the name', () => {
    const option = toCompletion(quoted);
    expect(option.label).toBe('"Person.id"');
    expect(option.displayLabel).toBe('Person.id');
    expect(toCompletion(rows[0]!).displayLabel).toBeUndefined();
  });

  it('replaces the quoted part, dots and all, and keeps the receiver', async () => {
    const source = fossilCompletionSource({ completion: () => [quoted] });
    const doc = 'x = KnowsRow."Person.i';
    const result = await source(contextAt(doc, doc.length));
    expect(doc.slice(result!.from)).toBe('"Person.i');
  });
});

describe('fossilCompletionSource', () => {
  const source = fossilCompletionSource({ completion: () => rows });

  it('replaces only the member, not the receiver that narrowed it', async () => {
    const doc = 'x = User.name.tr';
    const result = await source(contextAt(doc, doc.length));
    expect(result).not.toBeNull();
    // `tr` is what gets replaced — everything through the last dot stays.
    expect(doc.slice(result!.from)).toBe('tr');
  });

  it('fires the moment the dot is typed, with nothing to replace', async () => {
    const doc = 'x = User.';
    const result = await source(contextAt(doc, doc.length));
    expect(result!.from).toBe(doc.length);
  });

  it('does not fire on whitespace unless asked explicitly', async () => {
    const doc = 'x = ';
    expect(await source(contextAt(doc, doc.length))).toBeNull();
    expect(await source(contextAt(doc, doc.length, true))).not.toBeNull();
  });

  it('offers nothing when the compiler offers nothing', async () => {
    const empty = fossilCompletionSource({ completion: () => [] });
    const doc = 'x = User.na';
    expect(await empty(contextAt(doc, doc.length))).toBeNull();
  });

  it('answers a refused request with no list rather than a thrown error', async () => {
    const refusing = fossilCompletionSource({
      completion: () => {
        throw new Error('fossil workspace is busy');
      },
    });
    const doc = 'x = User.na';
    await expect(refusing(contextAt(doc, doc.length))).resolves.toBeNull();
  });

  it('does not set validFor, so every keystroke re-asks the compiler', async () => {
    // The compiler decided which rows apply at this cursor. A client-side
    // prefix filter over its answer is a second opinion from the side that
    // knows less — see the module header.
    const doc = 'x = User.na';
    const result = await source(contextAt(doc, doc.length));
    expect(result!.validFor).toBeUndefined();
  });
});
