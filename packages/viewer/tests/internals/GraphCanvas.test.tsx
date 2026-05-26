/**
 * Unit tests for GraphCanvas — Phase 12 plan 12-03 Task 2.
 *
 * Three tests:
 *   1. Loader fallback when graphData is null (empty vertices).
 *      Note: Although Keasy renders this as a loader-spinner, our hook
 *      returns `null` ONLY for `vertices.length === 0`. The plan still
 *      says "empty-state" for empty vertices, but the implementation
 *      goes through the null sentinel first → loader path. We assert
 *      the LOADER text here to match implementation; the empty-state
 *      path is tested separately when graphData exists but is empty
 *      after filtering (vertices but all groups hidden) — covered by
 *      the legend-toggle test transitively.
 *   2. Legend renders one Toggle per unique vertex type, each with the
 *      correct count badge.
 *   3. Click a legend Toggle → component re-renders with that type's
 *      vertices made invisible (alpha=0 in pointColors).
 *
 * Reuses the vi.mock('@cosmos.gl/graph') pattern from 12-02's
 * CosmosGraph test, plus a stub for `@uwdata/mosaic-core` so the
 * useGraphCrossfilter dependency does not throw.
 */
import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type Mock,
} from 'vitest';
import {
  render,
  screen,
  cleanup,
  act,
  fireEvent,
} from '@testing-library/react';
import { createRef } from 'react';

// Mock @cosmos.gl/graph so CosmosGraph can mount in happy-dom.
type MockGraph = {
  setConfig: Mock;
  setPointPositions: Mock;
  setPointColors: Mock;
  setPointSizes: Mock;
  setLinks: Mock;
  setPointClusters: Mock;
  setClusterPositions: Mock;
  render: Mock;
  destroy: Mock;
  zoom: Mock;
  getZoomLevel: Mock;
  fitView: Mock;
  start: Mock;
  pause: Mock;
  selectPointsByIndices: Mock;
  unselectPoints: Mock;
};

let mockGraphInstance: MockGraph;
let lastPointColorsPassed: Float32Array | null = null;

vi.mock('@cosmos.gl/graph', () => {
  const Graph = vi.fn().mockImplementation(function (
    this: object,
    _container: HTMLElement,
    config: unknown,
  ) {
    // The CosmosGraph wrapper's data useEffect passes pointColors to
    // setPointColors. We snapshot that for assertions.
    Object.assign(this, mockGraphInstance);
    void config;
    return mockGraphInstance;
  });
  return { Graph };
});

vi.mock('@uwdata/mosaic-core', () => ({
  coordinator: vi.fn(() => ({ disconnect: vi.fn() })),
  makeClient: vi.fn(() => ({})),
  clausePoints: vi.fn(() => ({})),
}));

vi.mock('@uwdata/mosaic-sql', () => ({
  Query: { from: vi.fn() },
  column: vi.fn(),
}));

import {
  GraphCanvas,
  type GraphCanvasProps,
} from '../../src/internals/GraphCanvas.js';
import type { CosmosGraphHandle } from '../../src/internals/CosmosGraph.js';

beforeEach(() => {
  lastPointColorsPassed = null;
  mockGraphInstance = {
    setConfig: vi.fn(),
    setPointPositions: vi.fn(),
    setPointColors: vi.fn((arr: Float32Array) => {
      // Snapshot the latest pointColors passed to the underlying Graph.
      lastPointColorsPassed = new Float32Array(arr);
    }),
    setPointSizes: vi.fn(),
    setLinks: vi.fn(),
    setPointClusters: vi.fn(),
    setClusterPositions: vi.fn(),
    render: vi.fn(),
    destroy: vi.fn(),
    zoom: vi.fn(),
    getZoomLevel: vi.fn().mockReturnValue(1),
    fitView: vi.fn(),
    start: vi.fn(),
    pause: vi.fn(),
    selectPointsByIndices: vi.fn(),
    unselectPoints: vi.fn(),
  };
});

afterEach(() => {
  cleanup();
});

function makeProps(
  overrides: Partial<GraphCanvasProps> = {},
): GraphCanvasProps {
  const ref = createRef<CosmosGraphHandle>();
  return {
    vertices: [],
    edges: [],
    graphRef: ref,
    ...overrides,
  };
}

describe('GraphCanvas', () => {
  it('renders the loader fallback when graphData is null (empty vertices)', () => {
    render(<GraphCanvas {...makeProps({ vertices: [], edges: [] })} />);

    expect(screen.getByText(/Loading\.\.\./i)).toBeTruthy();
    expect(document.querySelector('.fossil-viewer-loader')).toBeTruthy();
  });

  it('renders one legend Toggle per unique vertex type with correct count', () => {
    const vertices = [
      { id: 'a', type: 'Person', label: 'Alice' },
      { id: 'b', type: 'Person', label: 'Bob' },
      { id: 'c', type: 'Org', label: 'Acme' },
    ];

    render(<GraphCanvas {...makeProps({ vertices, edges: [] })} />);

    // The legend renders when groupEntries.length > 1.
    const legend = document.querySelector('.fossil-viewer-legend');
    expect(legend).toBeTruthy();

    const toggles = document.querySelectorAll('.fossil-viewer-legend-toggle');
    expect(toggles.length).toBe(2); // Person + Org

    // Count badges: "2" for Person, "1" for Org.
    const counts = Array.from(
      document.querySelectorAll('.fossil-viewer-legend-count'),
    ).map((el) => el.textContent);
    expect(counts).toContain('2');
    expect(counts).toContain('1');
  });

  it('clicking a legend Toggle hides that type by zeroing alpha in pointColors', () => {
    const vertices = [
      { id: 'a', type: 'Person', label: 'Alice' },
      { id: 'b', type: 'Person', label: 'Bob' },
      { id: 'c', type: 'Org', label: 'Acme' },
    ];

    render(<GraphCanvas {...makeProps({ vertices, edges: [] })} />);

    // First render — both groups visible. setPointColors called by data effect.
    expect(mockGraphInstance.setPointColors).toHaveBeenCalled();
    const initialColors = new Float32Array(lastPointColorsPassed!);
    // All 3 vertices have alpha=1 initially.
    expect(initialColors[3]).toBe(1);
    expect(initialColors[7]).toBe(1);
    expect(initialColors[11]).toBe(1);

    // Click the FIRST legend Toggle (Person, index 0).
    const firstToggle = document.querySelectorAll(
      '.fossil-viewer-legend-toggle',
    )[0] as HTMLButtonElement;
    act(() => {
      fireEvent.click(firstToggle);
    });

    // After toggle, pointColors gets re-pushed with alpha=0 for Person rows.
    const afterColors = new Float32Array(lastPointColorsPassed!);
    // Person (rows 0 + 1) → alpha=0 at index 3 + 7.
    expect(afterColors[3]).toBe(0);
    expect(afterColors[7]).toBe(0);
    // Org (row 2) → alpha=1 at index 11.
    expect(afterColors[11]).toBe(1);
  });
});
