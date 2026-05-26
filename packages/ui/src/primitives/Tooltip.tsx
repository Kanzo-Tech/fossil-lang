'use client';

/**
 * Tooltip primitive — wraps @radix-ui/react-tooltip with Fossil tokens.
 *
 * Re-exports: TooltipProvider (wraps Radix Provider with sensible
 * delayDuration/skipDelayDuration defaults), Tooltip (Root), TooltipTrigger,
 * TooltipPortal, TooltipContent (auto-wraps in Portal).
 *
 * Apps wrap their tree once at the root with `<TooltipProvider>`. Per
 * Radix's design, multiple Tooltip instances inside a single Provider share
 * skip-delay timing for adjacent hovers.
 *
 * Visual styling (bg, color, padding, radius) + the fade-in @keyframes
 * (`fossil-tooltip-fade-in` bound to `data-state="delayed-open"` and
 * `data-state="instant-open"`) live in the singleton stylesheet at
 * `../styles/inject.ts`. Inline style only carries z-index 60 so tooltips
 * stack above Dialog overlays (which currently render at z-auto in their
 * portal).
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-04.
 */

import * as React from 'react';
import * as TooltipPrimitive from '@radix-ui/react-tooltip';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

const tooltipContentStyle: React.CSSProperties = {
  // All visual styling (bg, color, padding, radius, animation) in inject.ts.
  // Inline style sets stacking order: tooltips render above Dialog overlays
  // (Dialog Content has no explicit z so it inherits portal stacking; we
  // budget z 60 here for visual hierarchy clarity).
  zIndex: 60,
};

export function TooltipProvider({
  delayDuration = 300,
  skipDelayDuration = 100,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      skipDelayDuration={skipDelayDuration}
      {...props}
    />
  );
}

export function Tooltip(
  props: React.ComponentProps<typeof TooltipPrimitive.Root>,
) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />;
}

export const TooltipTrigger = React.forwardRef<
  React.ElementRef<typeof TooltipPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Trigger>
>((props, ref) => (
  <TooltipPrimitive.Trigger
    ref={ref}
    data-slot="tooltip-trigger"
    {...props}
  />
));
TooltipTrigger.displayName = 'TooltipTrigger';

export function TooltipPortal(
  props: React.ComponentProps<typeof TooltipPrimitive.Portal>,
) {
  return (
    <TooltipPrimitive.Portal data-slot="tooltip-portal" {...props} />
  );
}

export const TooltipContent = React.forwardRef<
  React.ElementRef<typeof TooltipPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Content>
>(({ sideOffset = 4, style, ...props }, ref) => (
  <TooltipPrimitive.Portal>
    <TooltipPrimitive.Content
      ref={ref}
      data-slot="tooltip-content"
      data-fossil-ui-primitive=""
      sideOffset={sideOffset}
      style={{ ...tooltipContentStyle, ...style }}
      {...props}
    />
  </TooltipPrimitive.Portal>
));
TooltipContent.displayName = 'TooltipContent';
