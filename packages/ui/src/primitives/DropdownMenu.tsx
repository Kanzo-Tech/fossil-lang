'use client';

/**
 * DropdownMenu primitive — wraps @radix-ui/react-dropdown-menu with Fossil
 * tokens.
 *
 * Re-exports: DropdownMenu (Root), DropdownMenuTrigger, DropdownMenuPortal,
 * DropdownMenuContent (auto-wraps in Portal), DropdownMenuGroup,
 * DropdownMenuItem, DropdownMenuCheckboxItem, DropdownMenuRadioGroup,
 * DropdownMenuRadioItem, DropdownMenuLabel, DropdownMenuSeparator,
 * DropdownMenuSub, DropdownMenuSubTrigger, DropdownMenuSubContent.
 *
 * Most visual styling (background, padding, radius, item highlight) lives in
 * the singleton stylesheet (`../styles/inject.ts`) keyed off `data-slot`. The
 * Radix `[data-highlighted]` attribute drives the hover/keyboard-focus state
 * uniformly. Inline styles only carry structural positioning (z-index for
 * stacking above page content).
 *
 * Each subcomponent that owns a DOM element is wrapped in `React.forwardRef`
 * so Radix's internal Slot composition (used by `asChild` downstream) passes
 * refs cleanly. Per the pattern set in Plan 10-03.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-04.
 */

import * as React from 'react';
import * as DropdownMenuPrimitive from '@radix-ui/react-dropdown-menu';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

const contentStyle: React.CSSProperties = {
  // Visual styling (bg, border, radius, padding, shadow) handled by inject.ts
  // via [data-slot="dropdown-menu-content"]. Inline style is structural only:
  // ensure the content renders above page chrome.
  zIndex: 50,
};

export function DropdownMenu(
  props: React.ComponentProps<typeof DropdownMenuPrimitive.Root>,
) {
  return <DropdownMenuPrimitive.Root data-slot="dropdown-menu" {...props} />;
}

export const DropdownMenuTrigger = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Trigger>
>((props, ref) => (
  <DropdownMenuPrimitive.Trigger
    ref={ref}
    data-slot="dropdown-menu-trigger"
    {...props}
  />
));
DropdownMenuTrigger.displayName = 'DropdownMenuTrigger';

export function DropdownMenuPortal(
  props: React.ComponentProps<typeof DropdownMenuPrimitive.Portal>,
) {
  return (
    <DropdownMenuPrimitive.Portal
      data-slot="dropdown-menu-portal"
      {...props}
    />
  );
}

export function DropdownMenuGroup(
  props: React.ComponentProps<typeof DropdownMenuPrimitive.Group>,
) {
  return (
    <DropdownMenuPrimitive.Group data-slot="dropdown-menu-group" {...props} />
  );
}

export const DropdownMenuContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Content>
>(({ sideOffset = 4, style, ...props }, ref) => (
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.Content
      ref={ref}
      data-slot="dropdown-menu-content"
      data-fossil-ui-primitive=""
      sideOffset={sideOffset}
      style={{ ...contentStyle, ...style }}
      {...props}
    />
  </DropdownMenuPrimitive.Portal>
));
DropdownMenuContent.displayName = 'DropdownMenuContent';

export const DropdownMenuItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Item>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Item>
>((props, ref) => (
  <DropdownMenuPrimitive.Item
    ref={ref}
    data-slot="dropdown-menu-item"
    {...props}
  />
));
DropdownMenuItem.displayName = 'DropdownMenuItem';

export const DropdownMenuCheckboxItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.CheckboxItem>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.CheckboxItem>
>((props, ref) => (
  <DropdownMenuPrimitive.CheckboxItem
    ref={ref}
    data-slot="dropdown-menu-checkbox-item"
    {...props}
  />
));
DropdownMenuCheckboxItem.displayName = 'DropdownMenuCheckboxItem';

export function DropdownMenuRadioGroup(
  props: React.ComponentProps<typeof DropdownMenuPrimitive.RadioGroup>,
) {
  return (
    <DropdownMenuPrimitive.RadioGroup
      data-slot="dropdown-menu-radio-group"
      {...props}
    />
  );
}

export const DropdownMenuRadioItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.RadioItem>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.RadioItem>
>((props, ref) => (
  <DropdownMenuPrimitive.RadioItem
    ref={ref}
    data-slot="dropdown-menu-radio-item"
    {...props}
  />
));
DropdownMenuRadioItem.displayName = 'DropdownMenuRadioItem';

export const DropdownMenuLabel = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Label>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Label>
>((props, ref) => (
  <DropdownMenuPrimitive.Label
    ref={ref}
    data-slot="dropdown-menu-label"
    {...props}
  />
));
DropdownMenuLabel.displayName = 'DropdownMenuLabel';

export const DropdownMenuSeparator = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Separator>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Separator>
>((props, ref) => (
  <DropdownMenuPrimitive.Separator
    ref={ref}
    data-slot="dropdown-menu-separator"
    {...props}
  />
));
DropdownMenuSeparator.displayName = 'DropdownMenuSeparator';

export function DropdownMenuSub(
  props: React.ComponentProps<typeof DropdownMenuPrimitive.Sub>,
) {
  return (
    <DropdownMenuPrimitive.Sub data-slot="dropdown-menu-sub" {...props} />
  );
}

export const DropdownMenuSubTrigger = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.SubTrigger>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubTrigger>
>((props, ref) => (
  <DropdownMenuPrimitive.SubTrigger
    ref={ref}
    data-slot="dropdown-menu-sub-trigger"
    {...props}
  />
));
DropdownMenuSubTrigger.displayName = 'DropdownMenuSubTrigger';

export const DropdownMenuSubContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.SubContent>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubContent>
>(({ style, ...props }, ref) => (
  <DropdownMenuPrimitive.SubContent
    ref={ref}
    data-slot="dropdown-menu-sub-content"
    style={{ ...contentStyle, ...style }}
    {...props}
  />
));
DropdownMenuSubContent.displayName = 'DropdownMenuSubContent';
