'use client';

/**
 * Separator primitive — wraps @radix-ui/react-separator with Fossil tokens.
 *
 * Orientation = 'horizontal' (default) or 'vertical'. Renders a 1px line in
 * `--fossil-colors-border`. Sizes to its container (`width: 100%` for
 * horizontal, `height: 100%` for vertical).
 *
 * Per Radix default, `decorative=true` omits the ARIA role (the separator is
 * purely visual). Pass `decorative={false}` to mark it as a semantic
 * separator (`role="separator"`).
 *
 * Wrapped in `React.forwardRef` for parity with the other primitives so
 * consumers can compose with Radix's `asChild` pattern.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-03.
 */

import * as React from 'react';
import * as SeparatorPrimitive from '@radix-ui/react-separator';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

const baseStyle: React.CSSProperties = {
  background: 'var(--fossil-colors-border)',
  flexShrink: 0,
};

export const Separator = React.forwardRef<
  React.ElementRef<typeof SeparatorPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof SeparatorPrimitive.Root>
>(({ orientation = 'horizontal', decorative, style, ...props }, ref) => {
  const sizingStyle: React.CSSProperties =
    orientation === 'vertical'
      ? { width: '1px', height: '100%' }
      : { height: '1px', width: '100%' };
  return (
    <SeparatorPrimitive.Root
      ref={ref}
      data-slot="separator"
      data-orientation={orientation}
      orientation={orientation}
      decorative={decorative}
      style={{ ...baseStyle, ...sizingStyle, ...style }}
      {...props}
    />
  );
});
Separator.displayName = 'Separator';
