/**
 * Go to definition, and the branch that matters: **is the target in this
 * buffer**.
 *
 * Two of the four positions `fossil_ide::goto_definition` recognises resolve
 * into the shape document. A version of this that always moved the cursor would
 * work on the two in-file positions and put the caret at the shape document's
 * coordinates in the program for the other two — a jump that goes somewhere
 * plausible and wrong, which is the worst kind.
 */
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { describe, expect, it } from 'vitest';

import { gotoDefinitionAt, type DefinitionRowLike } from '../src/navigate.js';

const URI = 'hello.fossil';
const DOC = [
  'type { Person } := io.shex("hello.shex")',
  'User := io.csv("users.csv")',
  '',
  'People : Person from User',
  '    @subject = "https://example.org/user/{User.id}"',
  '    name     = User.name',
].join('\n');

function view(): EditorView {
  return new EditorView({ state: EditorState.create({ doc: DOC }) });
}

/** The source resolves once, then the assertions run on the microtask queue the
 *  extension schedules its dispatch on. */
const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

describe('gotoDefinitionAt', () => {
  it('moves the cursor when the target is this buffer', async () => {
    const v = view();
    const target: DefinitionRowLike = {
      uri: URI,
      range: { start: { line: 1, character: 0 }, end: { line: 1, character: 4 } },
    };
    let handed: DefinitionRowLike | null | undefined;
    gotoDefinitionAt(v, DOC.length - 4, () => [target], {
      uri: URI,
      onNavigate: (t) => {
        handed = t;
      },
    });
    await settle();
    expect(handed).toBeUndefined();
    expect(v.state.selection.main.head).toBe(v.state.doc.line(2).from);
    v.destroy();
  });

  it('hands a target in another file to the host, and does not move', async () => {
    const v = view();
    const before = v.state.selection.main.head;
    const target: DefinitionRowLike = {
      uri: 'hello.shex',
      range: { start: { line: 21, character: 2 }, end: { line: 21, character: 9 } },
    };
    let handed: DefinitionRowLike | null | undefined;
    gotoDefinitionAt(v, 8, () => [target], {
      uri: URI,
      onNavigate: (t) => {
        handed = t;
      },
    });
    await settle();
    expect(handed).toEqual(target);
    expect(v.state.selection.main.head).toBe(before);
    v.destroy();
  });

  it('says nothing-here rather than leaving the keypress looking broken', async () => {
    const v = view();
    let handed: DefinitionRowLike | null | undefined = undefined;
    gotoDefinitionAt(v, 0, () => [], {
      uri: URI,
      onNavigate: (t) => {
        handed = t;
      },
    });
    await settle();
    expect(handed).toBeNull();
    v.destroy();
  });

  it('reports a refused request the same way, instead of throwing into the keymap', async () => {
    const v = view();
    let handed: DefinitionRowLike | null | undefined = undefined;
    gotoDefinitionAt(
      v,
      0,
      () => {
        throw new Error('fossil workspace is busy');
      },
      {
        uri: URI,
        onNavigate: (t) => {
          handed = t;
        },
      },
    );
    await settle();
    expect(handed).toBeNull();
    v.destroy();
  });

  it('takes the first of several targets', async () => {
    const v = view();
    const first: DefinitionRowLike = {
      uri: 'a.shex',
      range: { start: { line: 1, character: 0 }, end: { line: 1, character: 1 } },
    };
    const second: DefinitionRowLike = {
      uri: 'b.shex',
      range: { start: { line: 2, character: 0 }, end: { line: 2, character: 1 } },
    };
    let handed: DefinitionRowLike | null | undefined;
    gotoDefinitionAt(v, 8, () => [first, second], {
      uri: URI,
      onNavigate: (t) => {
        handed = t;
      },
    });
    await settle();
    expect(handed).toEqual(first);
    v.destroy();
  });
});
