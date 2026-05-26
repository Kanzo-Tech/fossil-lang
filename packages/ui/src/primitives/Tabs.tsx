'use client';

/**
 * Tabs primitive — wraps @radix-ui/react-tabs with Fossil tokens.
 *
 * Variants on TabsList: 'default' (pill — bg `--fossil-colors-muted`, active
 * gets `--fossil-colors-background` + soft shadow) and 'line' (transparent
 * bg + underline indicator via ::after on active trigger).
 *
 * State-driven styling (`[data-state="active"]` / `:hover` /
 * `:focus-visible`) lives in the singleton stylesheet from `../styles/inject.ts`
 * (selectors keyed off `data-slot` + `data-variant`). Structural inline styles
 * (layout, sizing, padding) live here so consumers don't need to import any
 * CSS file.
 *
 * Each component is wrapped in `React.forwardRef` so Radix's internal Slot
 * composition (used by other primitives that wrap Tabs subparts via `asChild`)
 * can pass refs through cleanly without a "Function components cannot be
 * given refs" warning.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-03.
 */

import * as React from 'react';
import * as TabsPrimitive from '@radix-ui/react-tabs';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

export type TabsListVariant = 'default' | 'line';

const rootStyle: React.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: 'var(--fossil-spacing-2)',
};

function makeListStyle(variant: TabsListVariant): React.CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: variant === 'line' ? 'var(--fossil-spacing-1)' : '0',
    width: 'fit-content',
    height: 'var(--fossil-size-control-base)',
    padding: '3px',
    borderRadius: variant === 'line' ? '0' : 'var(--fossil-radii-lg)',
    backgroundColor:
      variant === 'line' ? 'transparent' : 'var(--fossil-colors-muted)',
    color: 'var(--fossil-colors-foreground)',
  };
}

const triggerStyle: React.CSSProperties = {
  position: 'relative',
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  gap: '6px',
  flex: 1,
  height: 'calc(100% - 1px)',
  padding: 'var(--fossil-spacing-1) var(--fossil-spacing-2)',
  borderRadius: 'var(--fossil-radii-md)',
  border: '1px solid transparent',
  fontSize: 'var(--fossil-fonts-sizeSmall)',
  fontWeight: 500,
  whiteSpace: 'nowrap',
  cursor: 'pointer',
  transition:
    'all var(--fossil-motion-duration-fast) var(--fossil-motion-easing)',
  background: 'transparent',
};

const contentStyle: React.CSSProperties = {
  flex: 1,
  minHeight: 0,
  display: 'flex',
  flexDirection: 'column',
  outline: 'none',
};

export const Tabs = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Root>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Root>
>(({ orientation = 'horizontal', style, ...props }, ref) => (
  <TabsPrimitive.Root
    ref={ref}
    data-slot="tabs"
    data-fossil-ui-primitive=""
    data-orientation={orientation}
    orientation={orientation}
    style={{ ...rootStyle, ...style }}
    {...props}
  />
));
Tabs.displayName = 'Tabs';

export interface TabsListProps
  extends React.ComponentPropsWithoutRef<typeof TabsPrimitive.List> {
  variant?: TabsListVariant;
}

export const TabsList = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.List>,
  TabsListProps
>(({ variant = 'default', style, ...props }, ref) => (
  <TabsPrimitive.List
    ref={ref}
    data-slot="tabs-list"
    data-variant={variant}
    style={{ ...makeListStyle(variant), ...style }}
    {...props}
  />
));
TabsList.displayName = 'TabsList';

export const TabsTrigger = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>
>(({ style, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    data-slot="tabs-trigger"
    style={{ ...triggerStyle, ...style }}
    {...props}
  />
));
TabsTrigger.displayName = 'TabsTrigger';

export const TabsContent = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Content>
>(({ style, ...props }, ref) => (
  <TabsPrimitive.Content
    ref={ref}
    data-slot="tabs-content"
    style={{ ...contentStyle, ...style }}
    {...props}
  />
));
TabsContent.displayName = 'TabsContent';
