'use client';

/**
 * Resizable primitives — wrap `react-resizable-panels` with Fossil tokens.
 *
 * Re-exports: ResizablePanelGroup (Group — flex container holding panels),
 * ResizablePanel (Panel — individual panel; re-exported as-is, no wrapper),
 * ResizableHandle (PanelResizeHandle — the drag handle between two panels;
 * supports an optional `withHandle` prop that renders a centred grip
 * indicator for affordance).
 *
 * Why `react-resizable-panels` instead of a Radix primitive: Radix UI does
 * not ship a resizable primitive. The Keasy reference uses
 * `react-resizable-panels` (Bvaughn's well-maintained library; MIT) — we
 * mirror that choice. ADR-0033 documents `react-resizable-panels` as the
 * single non-Radix primitive in `@fossil-lang/ui`.
 *
 * Visual styling (handle hover, grip indicator, layout direction) lives in
 * the singleton stylesheet at `../styles/inject.ts`. The library sets its
 * own `[data-resize-handle-state="hover|drag"]` attribute on the handle
 * during interaction; the stylesheet keys off both that attribute AND the
 * standard CSS `:hover` selector for parity with Keasy.
 *
 * Notes on typing:
 *   - `ResizablePanel` is re-exported directly from `react-resizable-panels`
 *     with no wrapper — the Panel element doesn't need `data-slot` (consumers
 *     don't style individual panels) and the library forwards refs cleanly.
 *   - `ResizablePanelGroup` is wrapped via `forwardRef` to attach
 *     `data-slot="resizable-panel-group"`. Its ref type is the library's
 *     `ImperativePanelGroupHandle` (NOT a DOM element) — exposed so consumers
 *     can imperatively call `.setLayout(...)` if they need to.
 *   - `ResizableHandle` is NOT wrapped in `forwardRef` because
 *     `PanelResizeHandle` is a plain function component in the library
 *     (no ref forwarding). Using `forwardRef` here would emit a TS error
 *     ("Property 'ref' does not exist") because Radix-style refs aren't
 *     supported by the underlying component.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-05.
 */

import * as React from 'react';
import {
  PanelGroup,
  Panel,
  PanelResizeHandle,
  type ImperativePanelGroupHandle,
  type PanelGroupProps,
  type PanelResizeHandleProps,
} from 'react-resizable-panels';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

// Explicit annotation (instead of relying on forwardRef inference) avoids
// TS2742 "The inferred type cannot be named without a reference to
// '.../declarations/src/types.js'" — composite which the consumer's
// declaration file would otherwise need to resolve via the library's deep
// internal types path. We pin both the ref handle and the props to the
// library's exported types so the emitted .d.ts is self-contained.
export const ResizablePanelGroup: React.ForwardRefExoticComponent<
  PanelGroupProps & React.RefAttributes<ImperativePanelGroupHandle>
> = React.forwardRef<ImperativePanelGroupHandle, PanelGroupProps>(
  (props, ref) => (
    // react-resizable-panels handles the `data-panel-group-direction`
    // attribute automatically based on the `direction` prop
    // ('horizontal' | 'vertical'). We only add
    // `data-slot="resizable-panel-group"` so inject.ts can key off it for
    // layout (display:flex + direction-aware flex-direction).
    <PanelGroup ref={ref} data-slot="resizable-panel-group" {...props} />
  ),
);
ResizablePanelGroup.displayName = 'ResizablePanelGroup';

// Re-exported as-is; the library forwards refs and consumers don't need
// data-slot styling on individual panels.
export { Panel as ResizablePanel };

export interface ResizableHandleProps extends PanelResizeHandleProps {
  /**
   * When `true`, renders a centred grip indicator inside the handle for
   * additional affordance. Useful for IDE-style layouts where the 1px
   * handle would otherwise be hard to discover. Mirrors the Keasy
   * `<ResizableHandle withHandle />` API.
   */
  withHandle?: boolean;
}

export function ResizableHandle({
  withHandle,
  children,
  ...props
}: ResizableHandleProps) {
  return (
    <PanelResizeHandle data-slot="resizable-handle" {...props}>
      {withHandle ? (
        // The grip indicator is purely decorative — screen readers announce
        // the resize handle via PanelResizeHandle's own ARIA wiring.
        <div data-slot="resizable-handle-grip" aria-hidden="true">
          ⋮
        </div>
      ) : null}
      {children}
    </PanelResizeHandle>
  );
}
ResizableHandle.displayName = 'ResizableHandle';
