/**
 * MappingPanel — the .fossil source editor pane.
 *
 * Wraps `<FossilEditor/>` with the WASM-ready gate (the editor's CodeMirror
 * StreamParser eagerly calls tokenize() on mount — rendering before
 * initFossilWasm resolves would crash; see Phase 8 plan 08-11). Lives inside
 * the IDE tabs layout's left panel (TabsContent value="mapping").
 *
 * Phase 14 plan 14-03. Pure presentational — receives the value/onChange
 * pair from the parent so tab switches (which unmount Radix TabsContent by
 * default) preserve the textual source via parent state.
 */

import { type ReactNode } from 'react';
import { FossilEditor } from '@fossil-lang/editor';
import type { Extension } from '@codemirror/state';
import { ARIA_LABELS } from '../a11y/index.js';

export interface MappingPanelProps {
  value: string;
  onChange: (v: string) => void;
  extensions: Extension[];
  wasmReady: boolean;
  /** Optional override for the not-yet-WASM-ready placeholder. */
  loadingFallback?: ReactNode;
}

/**
 * Render the Mapping tab content. When WASM hasn't finished initialising
 * the placeholder renders with a polite live region so screen readers
 * announce the loading state once; the parent component otherwise stays
 * interactive (toolbar buttons + other tabs).
 */
export function MappingPanel(props: MappingPanelProps): JSX.Element {
  const { value, onChange, extensions, wasmReady, loadingFallback } = props;
  // The aria-label region wraps BOTH states so landmark navigation surfaces
  // the editor pane even while WASM is still booting (matches pre-Phase-14
  // markup; load-bearing for the a11y test that asserts the editor section
  // exists at mount).
  return (
    <section aria-label={ARIA_LABELS.editor} data-testid="mapping-panel">
      {wasmReady ? (
        <FossilEditor
          value={value}
          onChange={onChange}
          extensions={extensions}
          // lspTransport is ignored when extensions is provided (the
          // playground pre-composes its own LSP wiring via useLspWorker).
          // Pass null to satisfy the LOCKED FossilEditorProps surface per
          // ADR-0036.
          lspTransport={null}
        />
      ) : loadingFallback != null ? (
        loadingFallback
      ) : (
        <div
          role="status"
          aria-live="polite"
          className="fossil-playground__editor-loading"
          style={{ padding: '1rem', color: 'var(--fossil-colors-muted)' }}
        >
          Loading editor…
        </div>
      )}
    </section>
  );
}
