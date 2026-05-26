'use client';

/**
 * Dialog primitive — wraps @radix-ui/react-dialog with Fossil tokens.
 *
 * Re-exports: Dialog (Root), DialogTrigger, DialogPortal, DialogOverlay,
 * DialogContent, DialogTitle, DialogDescription, DialogClose.
 *
 * Replaces direct `@radix-ui/react-dialog` consumption in the playground's
 * BibtexModal (Phase 9 plan 09-08) so dialogs are uniformly Fossil-themed
 * across the v0.2 surface.
 *
 * Open/close keyframes deferred per CONTEXT.md iteration-2 note — position
 * + bg + border + radius suffices for v0.2 baseline. Overlay positioning
 * + bg live in inject.ts via [data-slot="dialog-overlay"] selector.
 *
 * Each subcomponent is wrapped in `React.forwardRef` so Radix's internal
 * Slot composition (e.g. DialogPortal's Slot wrapping DialogOverlay /
 * DialogContent) can pass refs through cleanly.
 *
 * Per ADR-0033 + ADR-0034 + Phase 10 plan 10-03.
 */

import * as React from 'react';
import * as DialogPrimitive from '@radix-ui/react-dialog';
import { injectFossilUiStyles } from '../styles/inject.js';

// Inject the singleton stylesheet on module import (SSR-safe + idempotent).
injectFossilUiStyles();

const overlayStyle: React.CSSProperties = {
  // Background + positioning applied by inject.ts via [data-slot="dialog-overlay"]
  // — duplicated here for inline structural correctness when CSS hasn't
  // loaded yet (FOUC mitigation). The static stylesheet overrides if needed.
  position: 'fixed',
  inset: 0,
};

const contentStyle: React.CSSProperties = {
  position: 'fixed',
  top: '50%',
  left: '50%',
  transform: 'translate(-50%, -50%)',
  background: 'var(--fossil-colors-background)',
  color: 'var(--fossil-colors-foreground)',
  borderRadius: 'var(--fossil-radii-xl)',
  border: '1px solid var(--fossil-colors-border)',
  padding: 'var(--fossil-spacing-4)',
  minWidth: '320px',
  maxWidth: 'min(90vw, 600px)',
  boxShadow: '0 10px 30px rgba(0, 0, 0, 0.15)',
  outline: 'none',
};

const titleStyle: React.CSSProperties = {
  fontSize: 'var(--fossil-fonts-sizeBase)',
  fontWeight: 600,
  lineHeight: 1.2,
  margin: 0,
  marginBottom: 'var(--fossil-spacing-2)',
  color: 'var(--fossil-colors-foreground)',
};

const descriptionStyle: React.CSSProperties = {
  fontSize: 'var(--fossil-fonts-sizeSmall)',
  color: 'var(--fossil-colors-muted)',
  margin: 0,
  marginBottom: 'var(--fossil-spacing-3)',
};

const closeStyle: React.CSSProperties = {
  position: 'absolute',
  top: 'var(--fossil-spacing-3)',
  right: 'var(--fossil-spacing-3)',
  background: 'transparent',
  border: 'none',
  cursor: 'pointer',
  color: 'var(--fossil-colors-foreground)',
  fontSize: 'var(--fossil-fonts-sizeBase)',
  borderRadius: 'var(--fossil-radii-sm)',
  padding: 'var(--fossil-spacing-1)',
};

export function Dialog(
  props: React.ComponentProps<typeof DialogPrimitive.Root>,
) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

export const DialogTrigger = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Trigger>
>((props, ref) => (
  <DialogPrimitive.Trigger ref={ref} data-slot="dialog-trigger" {...props} />
));
DialogTrigger.displayName = 'DialogTrigger';

export function DialogPortal(
  props: React.ComponentProps<typeof DialogPrimitive.Portal>,
) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />;
}

export const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ style, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    data-slot="dialog-overlay"
    style={{ ...overlayStyle, ...style }}
    {...props}
  />
));
DialogOverlay.displayName = 'DialogOverlay';

export const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content>
>(({ style, ...props }, ref) => (
  <DialogPrimitive.Content
    ref={ref}
    data-slot="dialog-content"
    data-fossil-ui-primitive=""
    style={{ ...contentStyle, ...style }}
    {...props}
  />
));
DialogContent.displayName = 'DialogContent';

export const DialogTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ style, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    data-slot="dialog-title"
    style={{ ...titleStyle, ...style }}
    {...props}
  />
));
DialogTitle.displayName = 'DialogTitle';

export const DialogDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ style, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    data-slot="dialog-description"
    style={{ ...descriptionStyle, ...style }}
    {...props}
  />
));
DialogDescription.displayName = 'DialogDescription';

export const DialogClose = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Close>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Close>
>(({ style, ...props }, ref) => (
  <DialogPrimitive.Close
    ref={ref}
    data-slot="dialog-close"
    style={{ ...closeStyle, ...style }}
    {...props}
  />
));
DialogClose.displayName = 'DialogClose';
