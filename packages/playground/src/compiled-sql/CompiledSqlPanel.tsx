/**
 * CompiledSqlPanel — React component that renders the DuckDB SQL emitted by
 * `crates/fossil-codegen` for the current mapping. Read-only CodeMirror 6
 * view; the SQL flows in via the `sql` prop (the parent `<FossilPlayground/>`
 * subscribes to source edits + calls `FossilPlayground::compileFile()`).
 *
 * Foundation for PLAY-07 — the unique-differentiator panel surfaced in the
 * Phase 9 launch demo (vs RMLMapper / Morph-KGC which don't expose the
 * compiled SQL at all).
 *
 * Per 09-RESEARCH.md Pattern 3 + 09-CONTEXT.md locked decision:
 *   - Read-only via BOTH `EditorState.readOnly.of(true)` AND
 *     `EditorView.editable.of(false)` — belt + suspenders.
 *   - Live update: dispatch a single change spanning the whole doc when
 *     `sql` prop changes; no-op guard when the doc already matches so we
 *     don't reset the user's selection / scroll position (RESEARCH.md
 *     Pitfall: "avoid setText every keystroke" — even read-only).
 *
 * Theming: inherits FossilTheme via the `data-theme` attribute + the
 * `.fossil-compiled-sql-panel` CSS scope (host CSS reads
 * `[data-theme="dark"]` to flip colours). Component itself stays
 * theme-agnostic at the JS level (no inline colour styles).
 *
 * a11y: `role="region"` + `aria-label="Compiled DuckDB SQL"` so screen
 * readers announce the panel as a landmark; CodeMirror's content host owns
 * its own keyboard interaction (the surrounding ARIA stays out of CM's way).
 */

import { useEffect, useRef } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView, lineNumbers } from '@codemirror/view';
import { sql } from '@codemirror/lang-sql';
import { DuckDB } from './duckdb-dialect.js';

export interface CompiledSqlPanelProps {
  /**
   * The DuckDB SQL text to render. Pass empty string to clear. The parent
   * `<FossilPlayground/>` typically debounces (200 ms) before pushing a new
   * value so a burst of keystrokes coalesces into one update.
   */
  sql: string;
  /**
   * Theme inherited from the surrounding FossilTheme. Drives the
   * `data-theme` attribute used by host CSS to flip colours.
   */
  theme?: 'light' | 'dark';
}

/**
 * Read-only CodeMirror 6 view that renders DuckDB SQL with the DuckDB
 * dialect's keyword + builtin lists. Two `useEffect`s:
 *   (1) mount: create the `EditorView` once;
 *   (2) updates: dispatch a whole-doc replacement when `sql` differs from
 *       the current doc text. The no-op guard avoids needless dispatches
 *       (preserves selection state + scroll position on identical content).
 */
export function CompiledSqlPanel({
  sql: sqlText,
  theme = 'light',
}: CompiledSqlPanelProps): JSX.Element {
  const ref = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);

  // Effect 1 — create EditorView once on mount; destroy on unmount.
  useEffect(() => {
    if (!ref.current) return;
    const state = EditorState.create({
      doc: sqlText,
      extensions: [
        lineNumbers(),
        sql({ dialect: DuckDB }),
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
      ],
    });
    viewRef.current = new EditorView({ state, parent: ref.current });
    return () => {
      viewRef.current?.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional: mount once. `sqlText` updates are handled by effect 2.
  }, []);

  // Effect 2 — dispatch updates when sql prop changes. No-op guard prevents
  // selection-state churn when the doc is already in sync.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const cur = view.state.doc.toString();
    if (cur === sqlText) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: sqlText },
    });
  }, [sqlText]);

  return (
    <div
      ref={ref}
      className="fossil-compiled-sql-panel"
      data-theme={theme}
      data-testid="compiled-sql-panel"
      role="region"
      aria-label="Compiled DuckDB SQL"
    />
  );
}
