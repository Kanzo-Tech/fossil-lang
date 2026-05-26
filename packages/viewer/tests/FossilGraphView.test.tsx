/**
 * Unit tests for FossilGraphView — Phase 12 plan 12-04 Task 2.
 *
 * Four tests:
 *   1. webgl={false} → renders TabularFallback (auto-bypass detection).
 *   2. webgl omitted + happy-dom returns null for getContext('webgl2')
 *      → auto-fallback to TabularFallback (the default-detection path).
 *   3. webgl={true} + canvas.getContext mocked to return a non-null
 *      WebGL context → renders GraphCanvas (TabularFallback absent).
 *   4. Empty vertices + webgl={false} → TabularFallback empty-state.
 *
 * Mocks @cosmos.gl/graph + @uwdata/mosaic-core so GraphCanvas can
 * mount in happy-dom without hitting real WebGL or a coordinator.
 */
import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
} from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';

vi.mock('@cosmos.gl/graph', () => {
  const instance = {
    setConfig: vi.fn(),
    setPointPositions: vi.fn(),
    setPointColors: vi.fn(),
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
  return {
    Graph: vi.fn().mockImplementation(function (this: object) {
      Object.assign(this, instance);
      return instance;
    }),
  };
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

import { FossilGraphView } from '../src/FossilGraphView.js';

const sampleVertices = [
  { id: 'a', type: 'Person', label: 'Alice' },
  { id: 'b', type: 'Person', label: 'Bob' },
];

let originalGetContext: typeof HTMLCanvasElement.prototype.getContext;

beforeEach(() => {
  originalGetContext = HTMLCanvasElement.prototype.getContext;
});

afterEach(() => {
  HTMLCanvasElement.prototype.getContext = originalGetContext;
  cleanup();
});

describe('FossilGraphView', () => {
  it('renders TabularFallback when webgl={false} (explicit opt-out bypasses detection)', () => {
    render(
      <FossilGraphView
        vertices={sampleVertices}
        edges={[]}
        webgl={false}
      />,
    );

    expect(screen.getByTestId('fossil-viewer-tabular-fallback')).toBeTruthy();
    // GraphCanvas's loader/canvas wrapper should NOT be present.
    expect(document.querySelector('.fossil-viewer-canvas-root')).toBeNull();
  });

  it('auto-falls back to TabularFallback when WebGL2 is unavailable (happy-dom default)', () => {
    // The 12-01 tests/setup.ts stub returns null from getContext — happy-dom
    // has no WebGL. With webgl omitted (default true), detectWebGL() returns
    // false and the fallback renders.
    render(<FossilGraphView vertices={sampleVertices} edges={[]} />);

    expect(screen.getByTestId('fossil-viewer-tabular-fallback')).toBeTruthy();
  });

  it('renders GraphCanvas when webgl={true} AND canvas.getContext returns non-null', () => {
    // Override the global stub for this test only.
    HTMLCanvasElement.prototype.getContext = function getContextStub() {
      return {} as unknown as RenderingContext;
    } as typeof HTMLCanvasElement.prototype.getContext;

    render(
      <FossilGraphView
        vertices={sampleVertices}
        edges={[]}
        webgl={true}
      />,
    );

    // TabularFallback should NOT be in the DOM.
    expect(screen.queryByTestId('fossil-viewer-tabular-fallback')).toBeNull();
    // GraphCanvas's canvas-root wrapper should be present.
    expect(document.querySelector('.fossil-viewer-canvas-root')).toBeTruthy();
  });

  it('renders the TabularFallback empty-state when vertices is empty and webgl={false}', () => {
    render(<FossilGraphView vertices={[]} edges={[]} webgl={false} />);
    expect(screen.getByTestId('fossil-viewer-tabular-fallback')).toBeTruthy();
    expect(screen.getByText(/No vertices\./)).toBeTruthy();
  });
});
