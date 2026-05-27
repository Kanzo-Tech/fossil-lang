/**
 * ShapePanel — the ShEx target-shape editor pane.
 *
 * Lives inside the IDE tabs layout's left panel (TabsContent value="shape").
 * Renders a `<FossilEditor/>` with `lspTransport={null}` because the LSP
 * worker speaks .fossil syntax, not ShEx — disabling the transport keeps
 * the editor functional (typing, scrolling, syntax-agnostic highlighting)
 * without surfacing irrelevant diagnostics.
 *
 * Write-capable (v0.2 decision per 14-03-PLAN): the user can edit the shape;
 * `setShex` is wired through to the parent so the permalink round-trips it.
 * If a future plan wants strict read-only mode, add
 * `extensions={[EditorView.editable.of(false)]}`.
 *
 * Phase 14 plan 14-03.
 */

import { FossilEditor } from '@fossil-lang/editor';
import type { ConnectionResolver } from '@fossil-lang/types';

export interface ShapePanelProps {
  shex: string | undefined;
  onChange: (v: string) => void;
  resolver: ConnectionResolver;
  wasmReady: boolean;
}

export function ShapePanel(props: ShapePanelProps): JSX.Element {
  const { shex, onChange, resolver, wasmReady } = props;
  if (!wasmReady) {
    return (
      <div
        role="status"
        aria-live="polite"
        style={{ padding: '1rem', color: 'var(--fossil-colors-muted)' }}
      >
        Loading shape editor…
      </div>
    );
  }
  return (
    <section aria-label="ShEx target shape" data-testid="shape-panel">
      <FossilEditor
        value={shex ?? ''}
        onChange={onChange}
        lspTransport={null}
        resolver={resolver}
      />
    </section>
  );
}
