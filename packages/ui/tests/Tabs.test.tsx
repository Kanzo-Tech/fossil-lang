/**
 * Tabs primitive — unit tests. Covers render, data-slot attributes, variant
 * prop (default + line), token consumption, and ARIA inheritance from Radix.
 *
 * Per Phase 10 plan 10-03 Task 2.
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from '../src/primitives/Tabs.js';

describe('Tabs', () => {
  it('renders all parts with data-slot attributes', () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
          <TabsTrigger value="b">B</TabsTrigger>
        </TabsList>
        <TabsContent value="a">Panel A</TabsContent>
        <TabsContent value="b">Panel B</TabsContent>
      </Tabs>,
    );
    expect(document.querySelector('[data-slot="tabs"]')).not.toBeNull();
    expect(document.querySelector('[data-slot="tabs-list"]')).not.toBeNull();
    expect(document.querySelectorAll('[data-slot="tabs-trigger"]')).toHaveLength(2);
    expect(screen.getByText('Panel A')).toBeTruthy();
  });

  it("supports variant='line'", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList variant="line">
          <TabsTrigger value="a">A</TabsTrigger>
        </TabsList>
        <TabsContent value="a">A</TabsContent>
      </Tabs>,
    );
    expect(
      document
        .querySelector('[data-slot="tabs-list"]')
        ?.getAttribute('data-variant'),
    ).toBe('line');
  });

  it("variant='default' is the default", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
        </TabsList>
        <TabsContent value="a">A</TabsContent>
      </Tabs>,
    );
    expect(
      document
        .querySelector('[data-slot="tabs-list"]')
        ?.getAttribute('data-variant'),
    ).toBe('default');
  });

  it('TabsList inline style references --fossil-* tokens (not Tailwind classes)', () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
        </TabsList>
        <TabsContent value="a">A</TabsContent>
      </Tabs>,
    );
    const list = document.querySelector('[data-slot="tabs-list"]') as HTMLElement;
    expect(list.getAttribute('style') || '').toMatch(/var\(--fossil-/);
    // Tabs root carries data-fossil-ui-primitive so the generic focus-visible
    // selector from inject.ts applies inside it.
    const root = document.querySelector('[data-slot="tabs"]') as HTMLElement;
    expect(root.hasAttribute('data-fossil-ui-primitive')).toBe(true);
  });

  it('has ARIA role="tablist" inherited from Radix', () => {
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
        </TabsList>
        <TabsContent value="a">A</TabsContent>
      </Tabs>,
    );
    expect(screen.getByRole('tablist')).toBeTruthy();
    expect(screen.getByRole('tab')).toBeTruthy();
  });

  it('singleton stylesheet is injected exactly once on module import', () => {
    // Multiple renders should not duplicate the <style> tag.
    render(
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">A</TabsTrigger>
        </TabsList>
        <TabsContent value="a">A</TabsContent>
      </Tabs>,
    );
    render(
      <Tabs defaultValue="b">
        <TabsList variant="line">
          <TabsTrigger value="b">B</TabsTrigger>
        </TabsList>
        <TabsContent value="b">B</TabsContent>
      </Tabs>,
    );
    const styleTags = document.querySelectorAll('#fossil-ui-primitive-styles');
    expect(styleTags).toHaveLength(1);
  });
});
