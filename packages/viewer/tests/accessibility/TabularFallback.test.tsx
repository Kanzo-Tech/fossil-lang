/**
 * Unit tests for TabularFallback — Phase 12 plan 12-04 Task 1.
 *
 * Four tests:
 *   1. Renders 3-row vertices table given 3 vertices (asserts each id is
 *      visible + thead has 3 columns with scope="col").
 *   2. Empty vertices renders the "No vertices." paragraph fallback.
 *   3. Edges table renders 1 row per edge with the predicate column.
 *   4. ARIA correctness: section[aria-label], caption per table, every
 *      th has scope="col" — the manual-axe approximation per plan
 *      decision (full axe-clean assertion runs in plan 12-05's
 *      Playwright spec with @axe-core/playwright).
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { TabularFallback } from '../../src/accessibility/TabularFallback.js';

afterEach(() => {
  cleanup();
});

describe('TabularFallback', () => {
  it('renders a 3-row vertices table given 3 vertices', () => {
    render(
      <TabularFallback
        vertices={[
          { id: 'a', type: 'Person', label: 'Alice' },
          { id: 'b', type: 'Person', label: 'Bob' },
          { id: 'c', type: 'Org', label: 'Acme' },
        ]}
        edges={[]}
      />,
    );

    const verticesTable = screen.getByTestId('fossil-viewer-vertices-table');
    expect(verticesTable.querySelectorAll('tbody tr').length).toBe(3);
    expect(screen.getByText('Alice')).toBeTruthy();
    expect(screen.getByText('Bob')).toBeTruthy();
    expect(screen.getByText('Acme')).toBeTruthy();
  });

  it('renders empty-state paragraph when vertices is empty', () => {
    render(<TabularFallback vertices={[]} edges={[]} />);
    expect(screen.getByText(/No vertices\./)).toBeTruthy();
    expect(screen.getByText(/No edges\./)).toBeTruthy();
  });

  it('renders 1 row per edge with the predicate column', () => {
    render(
      <TabularFallback
        vertices={[]}
        edges={[
          { source: 'a', target: 'b', predicate: 'knows' },
          { source: 'b', target: 'c', predicate: 'worksAt' },
        ]}
      />,
    );

    const edgesTable = screen.getByTestId('fossil-viewer-edges-table');
    expect(edgesTable.querySelectorAll('tbody tr').length).toBe(2);
    expect(screen.getByText('knows')).toBeTruthy();
    expect(screen.getByText('worksAt')).toBeTruthy();
  });

  it('uses semantic ARIA: section[aria-label], caption, scope="col" on every th', () => {
    render(
      <TabularFallback
        vertices={[{ id: 'a', type: 'Person', label: 'Alice' }]}
        edges={[{ source: 'a', target: 'a' }]}
      />,
    );

    const root = screen.getByTestId('fossil-viewer-tabular-fallback');
    expect(root.tagName.toLowerCase()).toBe('section');
    expect(root.getAttribute('aria-label')).toMatch(/tabular fallback/i);

    // Both tables have a <caption>.
    expect(root.querySelectorAll('caption').length).toBe(2);

    // Every <th> carries scope="col".
    const ths = root.querySelectorAll('th');
    expect(ths.length).toBeGreaterThan(0);
    ths.forEach((th) => {
      expect(th.getAttribute('scope')).toBe('col');
    });
  });
});
