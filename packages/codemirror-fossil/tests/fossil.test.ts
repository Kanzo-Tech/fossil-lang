/**
 * `fossil(program)` pushes the text from the view's update cycle, so every question the
 * extensions ask by position is about the document on screen.
 */
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import type { FossilProgram } from '@fossil-lang/wasm';
import { describe, expect, it } from 'vitest';

import { fossil } from '../src/index.js';

describe('fossil', () => {
  it('pushes the text at construction and on every change, before highlighting asks', () => {
    const calls: string[] = [];
    const program = {
      uri: 'p.fossil',
      update: (text: string) => calls.push(`update ${text}`),
      tokenize: () => [],
      semanticTokens: () => (calls.push('semanticTokens'), []),
      diagnostics: async () => [],
      hover: () => null,
      completion: () => [],
      definition: () => [],
    } as unknown as FossilProgram;
    const view = new EditorView({ state: EditorState.create({ doc: 'a', extensions: fossil(program) }) });
    view.dispatch({ changes: { from: 1, insert: 'b' } });
    expect(calls).toEqual(['update a', 'semanticTokens', 'update ab', 'semanticTokens']);
    view.destroy();
  });
});
