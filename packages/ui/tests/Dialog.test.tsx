/**
 * Dialog primitive — unit tests. Covers render, data-slot attributes, ARIA
 * dialog role inheritance from Radix, token consumption, and open/close
 * lifecycle.
 *
 * Per Phase 10 plan 10-03 Task 2.
 */

import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import {
  Dialog,
  DialogTrigger,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogClose,
} from '../src/primitives/Dialog.js';

function renderDialog(defaultOpen = false) {
  return render(
    <Dialog defaultOpen={defaultOpen}>
      <DialogTrigger>Open</DialogTrigger>
      <DialogPortal>
        <DialogOverlay />
        <DialogContent>
          <DialogTitle>Title text</DialogTitle>
          <DialogDescription>Description text</DialogDescription>
          <DialogClose>Close</DialogClose>
        </DialogContent>
      </DialogPortal>
    </Dialog>,
  );
}

describe('Dialog', () => {
  it('renders closed by default — trigger is in the DOM, content is not', () => {
    renderDialog(false);
    expect(document.querySelector('[data-slot="dialog-trigger"]')).not.toBeNull();
    expect(document.querySelector('[data-slot="dialog-content"]')).toBeNull();
  });

  it('opens on trigger click, exposing content with data-slot attributes', () => {
    renderDialog(false);
    fireEvent.click(screen.getByText('Open'));
    expect(document.querySelector('[data-slot="dialog-content"]')).not.toBeNull();
    expect(document.querySelector('[data-slot="dialog-overlay"]')).not.toBeNull();
    expect(document.querySelector('[data-slot="dialog-title"]')).not.toBeNull();
    expect(document.querySelector('[data-slot="dialog-description"]')).not.toBeNull();
    expect(document.querySelector('[data-slot="dialog-close"]')).not.toBeNull();
  });

  it('content has ARIA role="dialog" inherited from Radix', () => {
    renderDialog(true);
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it('DialogContent inline style references --fossil-* tokens', () => {
    renderDialog(true);
    const content = document.querySelector(
      '[data-slot="dialog-content"]',
    ) as HTMLElement;
    expect(content.getAttribute('style') || '').toMatch(/var\(--fossil-/);
    expect(content.hasAttribute('data-fossil-ui-primitive')).toBe(true);
  });

  it('closes when DialogClose is clicked', () => {
    renderDialog(true);
    expect(document.querySelector('[data-slot="dialog-content"]')).not.toBeNull();
    fireEvent.click(screen.getByText('Close'));
    expect(document.querySelector('[data-slot="dialog-content"]')).toBeNull();
  });
});
