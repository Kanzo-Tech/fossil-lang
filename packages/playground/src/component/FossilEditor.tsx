/**
 * FossilEditor — hand-rolled React wrapper around `EditorView`.
 *
 * Per RESEARCH.md Pattern 4 (~50-LOC wrapper instead of `@uiw/react-codemirror`):
 * the React ecosystem's CodeMirror wrappers add little value beyond what a
 * tightly-controlled ref + two `useEffect` calls deliver, and they impose
 * their own version constraints on `@codemirror/*` that frequently conflict
 * with peerDep ranges (RESEARCH.md Pitfall 10).
 *
 * Lifecycle:
 *   1. First effect (mount): construct `EditorState` from initial value +
 *      extensions, instantiate `EditorView`, attach updateListener that
 *      forwards doc changes to `onChange`. Cleanup destroys the view.
 *   2. Second effect (value control): if the controlled `value` diverges
 *      from the editor's current doc (e.g. host reset the example), dispatch
 *      a single replace transaction. Skipped when they match — prevents
 *      cursor jitter on every keystroke.
 *
 * The `extensions` dep on the first effect means changing the extensions
 * array tears the editor down + rebuilds it. Consumers (`FossilPlayground`)
 * memoise the extensions array via `useMemo` to keep the editor stable
 * across renders.
 */

import { useEffect, useRef } from 'react';
import { EditorState, type Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';

export interface FossilEditorProps {
  /** Controlled doc content. The editor reflects this when it diverges from
   *  the live doc; otherwise local edits flow uninterrupted. */
  value: string;
  /** Forwarded on every doc change (debounced by CodeMirror's own update
   *  batching — typically once per keystroke). */
  onChange?: (value: string) => void;
  /** CodeMirror extensions to compose. Typically `fossil({ resolver })` + the
   *  LSP feature extensions from `@codemirror/lsp-client`. Memoise via
   *  `useMemo` in the parent to keep the editor stable across renders. */
  extensions: Extension[];
  /** Optional class for theming hooks. Defaults to `'fossil-editor'`. */
  className?: string;
}

export function FossilEditor({
  value,
  onChange,
  extensions,
  className,
}: FossilEditorProps): JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  // Stash the latest onChange in a ref so the mount-effect doesn't re-run when
  // the host passes a fresh closure on every render.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  // Mount/teardown effect — keyed on the extensions array identity. Consumers
  // memoise to keep this stable.
  useEffect(() => {
    if (!hostRef.current) return;
    const state = EditorState.create({
      doc: value,
      extensions: [
        ...extensions,
        EditorView.updateListener.of((u) => {
          if (u.docChanged) onChangeRef.current?.(u.state.doc.toString());
        }),
      ],
    });
    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // Deliberate: exclude `value` so typing doesn't tear down the editor.
    // Controlled updates flow through the second effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [extensions]);

  // Controlled value sync — only fires when the host changes value
  // externally (e.g., Reset, Load Example). Skipped when the host echoes
  // back the value we already emitted via onChange.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  }, [value]);

  return <div ref={hostRef} className={className ?? 'fossil-editor'} />;
}
