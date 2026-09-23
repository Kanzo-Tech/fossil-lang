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
import { describe, expect, it } from 'vitest';

import { fossilCompletionSource, toCompletion, type CompletionRowLike } from '../src/complete.js';

const rows: CompletionRowLike[] = [
  { label: 'trim', kind: 'function', detail: 'str.trim(String) -> String' },
  { label: 'upper', kind: 'function', detail: '' },
];

function contextAt(doc: string, pos: number, explicit = false): CompletionContext {
  return new CompletionContext(EditorState.create({ doc }), pos, explicit);
}

describe('toCompletion', () => {
  it('maps the LSP kind name onto CodeMirror’s vocabulary', () => {
    expect(toCompletion({ label: 'x', kind: 'function', detail: '' }).type).toBe('function');
    // LSP has `field` and CodeMirror does not; both of LSP's field-ish kinds
    // are CodeMirror's `property`.
    expect(toCompletion({ label: 'x', kind: 'field', detail: '' }).type).toBe('property');
    expect(toCompletion({ label: 'x', kind: 'property', detail: '' }).type).toBe('property');
  });

  it('leaves the type off for a kind it does not know, rather than guessing', () => {
    // A kind appended by a compiler newer than this host. No icon beats a wrong
    // icon, and it must not throw.
    expect(toCompletion({ label: 'x', kind: 'quasar', detail: '' }).type).toBeUndefined();
    expect(toCompletion({ label: 'x', kind: '', detail: '' }).type).toBeUndefined();
  });

  it('omits an empty detail instead of rendering a blank line', () => {
    expect(toCompletion({ label: 'x', kind: 'function', detail: '' }).detail).toBeUndefined();
    expect(toCompletion({ label: 'x', kind: 'function', detail: 'sig' }).detail).toBe('sig');
  });
});

describe('fossilCompletionSource', () => {
  const source = fossilCompletionSource(() => rows);

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
    const empty = fossilCompletionSource(() => []);
    const doc = 'x = User.na';
    expect(await empty(contextAt(doc, doc.length))).toBeNull();
  });

  it('answers a refused request with no list rather than a thrown error', async () => {
    const refusing = fossilCompletionSource(() => {
      throw new Error('fossil workspace is busy');
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
