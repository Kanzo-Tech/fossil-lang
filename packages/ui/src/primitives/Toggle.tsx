'use client';

/**
 * Toggle + ToggleGroup primitives — wrap @radix-ui/react-toggle +
 * @radix-ui/react-toggle-group with Fossil tokens.
 *
 * Re-exports: Toggle (standalone pressable button — Keasy parity), ToggleGroup
 * (Root supporting `type="single" | "multiple"`), ToggleGroupItem.
 *
 * Variant model — Keasy ships `default` (transparent bg) and `outline`
 * (1px border in `--fossil-colors-border`). We re-implement the same two
 * variants via the `data-variant="default|outline"` attribute; visual styling
 * (bg/hover/pressed/outline border) lives in the singleton stylesheet at
 * `../styles/inject.ts`. No CVA, no Tailwind.
 *
 * ToggleGroup propagates the active variant to all children via a tiny
 * React context — consumers can set `<ToggleGroup variant="outline">` once
 * and every <ToggleGroupItem> inherits it without having to spread the prop
 * manually. The Keasy reference does the same pattern.
 *
 * Intentional design choice: <ToggleGroupItem> renders `data-slot="toggle"`
 * (NOT `data-slot="toggle-group-item"`) so the existing Toggle CSS selectors
 * in inject.ts apply to both standalone and grouped toggles. This keeps the
 * stylesheet small and the visual treatment uniform — a Toggle is a Toggle
 * regardless of whether it lives in a ToggleGroup. The grouping container
 * gets its own `data-slot="toggle-group"` for layout (gap + flex).
 *
 * Note on ToggleGroup typing: Radix's `ToggleGroupPrimitive.Root` accepts a
 * discriminated union `(ToggleGroupSingleProps | ToggleGroupMultipleProps)`.
 * TypeScript interfaces cannot `extends` a union, so the props type for our
 * <ToggleGroup> is a type alias (intersection) rather than an interface.
 *
 * Each subcomponent that owns a DOM element is wrapped in `React.forwardRef`
 * for parity with the other primitives so Radix's internal Slot composition
 * (used by `asChild` downstream) passes refs cleanly.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-05.
 */

import * as React from 'react';
import * as TogglePrimitive from '@radix-ui/react-toggle';
import * as ToggleGroupPrimitive from '@radix-ui/react-toggle-group';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

export type ToggleVariant = 'default' | 'outline';

export interface ToggleProps
  extends React.ComponentPropsWithoutRef<typeof TogglePrimitive.Root> {
  variant?: ToggleVariant;
}

export const Toggle = React.forwardRef<
  React.ElementRef<typeof TogglePrimitive.Root>,
  ToggleProps
>(({ variant = 'default', ...props }, ref) => (
  <TogglePrimitive.Root
    ref={ref}
    data-slot="toggle"
    data-variant={variant}
    {...props}
  />
));
Toggle.displayName = 'Toggle';

// Context propagating the variant from <ToggleGroup> to every
// <ToggleGroupItem>. Children read this so a single prop on the group sets
// the visual treatment for all items — same UX as the Keasy reference.
const ToggleGroupVariantContext = React.createContext<ToggleVariant>('default');

// Discriminated-union prop type (interfaces cannot extend a union). The
// intersection adds our optional `variant` prop on top of Radix's
// single|multiple Root props without losing the union discrimination.
export type ToggleGroupProps = React.ComponentPropsWithoutRef<
  typeof ToggleGroupPrimitive.Root
> & {
  variant?: ToggleVariant;
};

export const ToggleGroup = React.forwardRef<
  React.ElementRef<typeof ToggleGroupPrimitive.Root>,
  ToggleGroupProps
>(({ variant = 'default', children, ...props }, ref) => (
  // The cast is required because TypeScript can't narrow the discriminated
  // union after destructuring `variant` out — `...props` keeps the union
  // alive but the inference loses track. Runtime behaviour is identical
  // (variant is ours, type=single|multiple flows untouched to Radix).
  <ToggleGroupPrimitive.Root
    ref={ref}
    data-slot="toggle-group"
    data-variant={variant}
    {...(props as React.ComponentPropsWithoutRef<typeof ToggleGroupPrimitive.Root>)}
  >
    <ToggleGroupVariantContext.Provider value={variant}>
      {children}
    </ToggleGroupVariantContext.Provider>
  </ToggleGroupPrimitive.Root>
));
ToggleGroup.displayName = 'ToggleGroup';

export interface ToggleGroupItemProps
  extends React.ComponentPropsWithoutRef<typeof ToggleGroupPrimitive.Item> {
  /**
   * Optional per-item variant override. When omitted, the item inherits the
   * variant from the enclosing <ToggleGroup>. Useful in the rare case where
   * one item in a group needs a different treatment (uncommon — most groups
   * have uniform variant across items).
   */
  variant?: ToggleVariant;
}

export const ToggleGroupItem = React.forwardRef<
  React.ElementRef<typeof ToggleGroupPrimitive.Item>,
  ToggleGroupItemProps
>(({ variant, ...props }, ref) => {
  const contextVariant = React.useContext(ToggleGroupVariantContext);
  const resolved = variant ?? contextVariant;
  return (
    <ToggleGroupPrimitive.Item
      ref={ref}
      // Reuse Toggle's data-slot so the inject.ts selectors apply uniformly
      // to standalone Toggle AND ToggleGroup items — single source of CSS
      // truth. Documented in module-level JSDoc.
      data-slot="toggle"
      data-variant={resolved}
      {...props}
    />
  );
});
ToggleGroupItem.displayName = 'ToggleGroupItem';
