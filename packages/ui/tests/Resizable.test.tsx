/**
 * Resizable primitives — unit tests. Covers render of the panel group +
 * panels + handle, data-slot anatomy, `withHandle` conditional grip
 * indicator, and direction prop propagation via the underlying
 * `react-resizable-panels` `data-panel-group-direction` attribute.
 *
 * Per Phase 10 plan 10-05 Task 2.
 *
 * Note: We don't assert actual drag/resize behaviour — happy-dom doesn't
 * fire pointer events with full coordinate semantics, and
 * `react-resizable-panels` already tests its own drag math. We test the
 * DOM structure + the attributes our inject.ts CSS keys off.
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '../src/primitives/Resizable.js';

describe('Resizable', () => {
  it('renders group + 2 panels + handle with data-slot attributes', () => {
    render(
      <ResizablePanelGroup direction="horizontal">
        <ResizablePanel defaultSize={50}>Left</ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={50}>Right</ResizablePanel>
      </ResizablePanelGroup>,
    );
    expect(
      document.querySelector('[data-slot="resizable-panel-group"]'),
    ).not.toBeNull();
    expect(
      document.querySelector('[data-slot="resizable-handle"]'),
    ).not.toBeNull();
  });

  it('group exposes horizontal direction via data-panel-group-direction (set by react-resizable-panels)', () => {
    render(
      <ResizablePanelGroup direction="horizontal">
        <ResizablePanel defaultSize={50}>Left</ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={50}>Right</ResizablePanel>
      </ResizablePanelGroup>,
    );
    const group = document.querySelector(
      '[data-slot="resizable-panel-group"]',
    ) as HTMLElement;
    expect(group.getAttribute('data-panel-group-direction')).toBe('horizontal');
  });

  it('vertical direction propagates to the group', () => {
    render(
      <ResizablePanelGroup direction="vertical">
        <ResizablePanel defaultSize={50}>Top</ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={50}>Bottom</ResizablePanel>
      </ResizablePanelGroup>,
    );
    const group = document.querySelector(
      '[data-slot="resizable-panel-group"]',
    ) as HTMLElement;
    expect(group.getAttribute('data-panel-group-direction')).toBe('vertical');
  });

  it('withHandle={true} renders the grip indicator inside the handle', () => {
    render(
      <ResizablePanelGroup direction="horizontal">
        <ResizablePanel defaultSize={50}>Left</ResizablePanel>
        <ResizableHandle withHandle />
        <ResizablePanel defaultSize={50}>Right</ResizablePanel>
      </ResizablePanelGroup>,
    );
    const grip = document.querySelector(
      '[data-slot="resizable-handle-grip"]',
    );
    expect(grip).not.toBeNull();
    // The grip is aria-hidden because PanelResizeHandle already exposes the
    // ARIA contract for the resizer.
    expect(grip?.getAttribute('aria-hidden')).toBe('true');
  });

  it('withHandle omitted (default) does NOT render the grip indicator', () => {
    render(
      <ResizablePanelGroup direction="horizontal">
        <ResizablePanel defaultSize={50}>Left</ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={50}>Right</ResizablePanel>
      </ResizablePanelGroup>,
    );
    expect(
      document.querySelector('[data-slot="resizable-handle-grip"]'),
    ).toBeNull();
  });

  it('co-exists with the singleton stylesheet (exactly one #fossil-ui-primitive-styles)', () => {
    render(
      <ResizablePanelGroup direction="horizontal">
        <ResizablePanel defaultSize={50}>Left</ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={50}>Right</ResizablePanel>
      </ResizablePanelGroup>,
    );
    const styleTags = document.querySelectorAll('#fossil-ui-primitive-styles');
    expect(styleTags).toHaveLength(1);
  });
});
