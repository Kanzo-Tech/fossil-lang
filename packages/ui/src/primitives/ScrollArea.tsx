'use client';

/**
 * ScrollArea primitive — wraps @radix-ui/react-scroll-area with Fossil tokens.
 *
 * Re-exports: ScrollArea (Root — auto-renders Viewport + vertical ScrollBar
 * + Corner), ScrollBar (orientation-aware scrollbar — vertical default; pass
 * `orientation="horizontal"` to render a horizontal bar in addition).
 *
 * Visual styling (overflow positioning, scrollbar sizing, visible-on-hover
 * pattern, thumb color) lives in the singleton stylesheet at
 * `../styles/inject.ts`. The Radix `data-state="visible|hidden"` attributes
 * drive the opacity transition (default opacity 0; opacity 1 on Root hover
 * OR when state="visible"). Vertical/horizontal orientation is differentiated
 * via `[data-orientation="..."]` selectors.
 *
 * Inline styles only set viewport width:100% / height:100% so scrolling fills
 * the Root container — Radix needs this contract for its scroll math.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-04.
 */

import * as React from 'react';
import * as ScrollAreaPrimitive from '@radix-ui/react-scroll-area';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

const viewportStyle: React.CSSProperties = {
  // Radix requires width/height:100% on the Viewport so its scroll math
  // measures the full Root. Inject.ts also sets these as a defence-in-depth
  // (FOUC mitigation if the stylesheet has not yet been injected).
  width: '100%',
  height: '100%',
};

export const ScrollArea = React.forwardRef<
  React.ElementRef<typeof ScrollAreaPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof ScrollAreaPrimitive.Root>
>(({ children, ...props }, ref) => (
  <ScrollAreaPrimitive.Root ref={ref} data-slot="scroll-area" {...props}>
    <ScrollAreaPrimitive.Viewport
      data-slot="scroll-area-viewport"
      style={viewportStyle}
    >
      {children}
    </ScrollAreaPrimitive.Viewport>
    <ScrollBar />
    <ScrollAreaPrimitive.Corner data-slot="scroll-area-corner" />
  </ScrollAreaPrimitive.Root>
));
ScrollArea.displayName = 'ScrollArea';

export const ScrollBar = React.forwardRef<
  React.ElementRef<typeof ScrollAreaPrimitive.ScrollAreaScrollbar>,
  React.ComponentPropsWithoutRef<
    typeof ScrollAreaPrimitive.ScrollAreaScrollbar
  >
>(({ orientation = 'vertical', ...props }, ref) => (
  <ScrollAreaPrimitive.ScrollAreaScrollbar
    ref={ref}
    data-slot="scroll-area-scrollbar"
    data-orientation={orientation}
    orientation={orientation}
    {...props}
  >
    <ScrollAreaPrimitive.ScrollAreaThumb data-slot="scroll-area-thumb" />
  </ScrollAreaPrimitive.ScrollAreaScrollbar>
));
ScrollBar.displayName = 'ScrollBar';
