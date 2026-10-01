/**
 * `CheckRow` → CodeMirror `Diagnostic`. The interesting cases are the ones where
 * the row and the document disagree, because that is not an edge case — it is
 * every keystroke between a check being requested and its answer arriving.
 */
import { forEachDiagnostic, forceLinting } from '@codemirror/lint';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { FossilError, TITLES, helpUrl, type Problem } from '@fossil-lang/types';
import { describe, expect, it } from 'vitest';

import { fossilLinter, toDiagnostics, type CheckRowLike, type CheckSource } from '../src/lint.js';

const URI = 'hello.fossil';

const DOC = ['User := io.csv("users.csv")', '', 'Person : PersonShape from User', '  name = User.nmae'].join(
  '\n',
);

function state(doc = DOC): EditorState {
  return EditorState.create({ doc });
}

function row(over: Partial<CheckRowLike> = {}): CheckRowLike {
  return {
    uri: URI,
    range: { start: { line: 3, character: 14 }, end: { line: 3, character: 18 } },
    severity: 1,
    message: 'unknown column `nmae`',
    ...over,
  };
}

describe('toDiagnostics', () => {
  it('lands the span on the text the row names', () => {
    const s = state();
    const [d] = toDiagnostics(s, [row()], URI);
    expect(d).toBeDefined();
    expect(s.doc.sliceString(d!.from, d!.to)).toBe('nmae');
    expect(d!.severity).toBe('error');
    expect(d!.source).toBe('fossil');
  });

  it('maps every LSP severity, and treats an unknown one as an error', () => {
    const s = state();
    const sev = (n: number) => toDiagnostics(s, [row({ severity: n })], URI)[0]!.severity;
    expect(sev(1)).toBe('error');
    expect(sev(2)).toBe('warning');
    expect(sev(3)).toBe('info');
    expect(sev(4)).toBe('hint');
    expect(sev(9)).toBe('error');
  });

  it('drops rows belonging to another open file', () => {
    // `check()` is workspace-wide: a host opens `hello.shex` too, and its
    // rows must not be painted onto the program's text.
    const rows = [row(), row({ uri: 'hello.shex', message: 'shape is unsatisfiable' })];
    expect(toDiagnostics(state(), rows, URI)).toHaveLength(1);
  });

  it('folds related locations into the message rather than dropping them', () => {
    const [d] = toDiagnostics(
      state(),
      [
        row({
          related: [
            {
              uri: 'hello.shex',
              range: { start: { line: 6, character: 2 }, end: { line: 6, character: 9 } },
              message: 'declared here',
            },
          ],
        }),
      ],
      URI,
    );
    expect(d!.message).toContain('unknown column `nmae`');
    expect(d!.message).toContain('hello.shex:7');
    expect(d!.message).toContain('declared here');
  });

  it('clamps a row that points past the end of a shrunken document', () => {
    // The user deleted three lines while the check was in flight. CodeMirror
    // throws on an out-of-range decoration; an editor must not.
    const s = state('User := io.csv("u.csv")');
    const [d] = toDiagnostics(s, [row()], URI);
    expect(d!.from).toBeGreaterThanOrEqual(0);
    expect(d!.to).toBeLessThanOrEqual(s.doc.length);
    expect(d!.from).toBeLessThanOrEqual(d!.to);
  });

  it('widens a zero-width range so an error at end-of-line is visible', () => {
    const s = state();
    const [d] = toDiagnostics(
      s,
      [row({ range: { start: { line: 0, character: 3 }, end: { line: 0, character: 3 } } })],
      URI,
    );
    expect(d!.to).toBeGreaterThan(d!.from);
  });

  it('does not widen past the end of an empty document', () => {
    const s = state('');
    const [d] = toDiagnostics(
      s,
      [row({ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } } })],
      URI,
    );
    expect(d!.from).toBe(0);
    expect(d!.to).toBe(0);
  });
});

describe('a failure, as a diagnostic', () => {
  const unread: Problem = {
    code: 'storage/host-silent',
    data: { scope: 'its connections', after: 30_000 },
    title: TITLES['storage/host-silent'],
    detail: 'the host did not answer its connections within 30000 ms',
    severity: 'error',
  };

  it('shows a row’s problem by its title, its code and its page', () => {
    const [d] = toDiagnostics(state(), [row({ problem: unread })], URI);
    expect(d!.message.split('\n')[0]).toContain('[storage/host-silent]');
    expect(d!.message).toContain(helpUrl('storage/host-silent'));
  });

  it('shows a refused check by its code, and one fossil did not raise as internal/bug', async () => {
    const lint = async (source: CheckSource): Promise<string[]> => {
      const view = new EditorView({ state: EditorState.create({ doc: DOC, extensions: fossilLinter(source, { uri: URI, delay: 0 }) }) });
      forceLinting(view);
      await new Promise((settle) => setTimeout(settle, 50));
      const messages: string[] = [];
      forEachDiagnostic(view.state, (d) => messages.push(d.message));
      view.destroy();
      return messages;
    };
    expect((await lint(() => Promise.reject(FossilError.from(unread))))[0]).toContain('[storage/host-silent]');
    expect((await lint(() => Promise.reject(new TypeError('worker died'))))[0]).toContain('[internal/bug]');
  });
});
