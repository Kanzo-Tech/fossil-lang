/**
 * Unit tests for CosmosGraph — Phase 12 plan 12-02.
 *
 * Three test cases:
 *   1. Mount/unmount lifecycle (Graph constructor called once + destroy
 *      on unmount).
 *   2. WebGL-error fallback path (Graph constructor throws → fallback
 *      div renders → Retry button flips state).
 *   3. Imperative ref handle exposes the 8-method surface and dispatches
 *      to the underlying Graph instance.
 *
 * Test environment note: happy-dom returns `null` from
 * `canvas.getContext('webgl2')` — Cosmos.gl's `Graph` constructor would
 * therefore throw in unit tests. We mock the `@cosmos.gl/graph` module
 * so the success path can be exercised; Test 2 overrides the mock to
 * throw and verify the fallback path.
 *
 * The mock pattern established here is reusable for 12-03's GraphCanvas
 * tests — `vi.mock('@cosmos.gl/graph', factory)` at module top, with
 * `mockGraphInstance` reset in beforeEach.
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
import { render, screen, cleanup, act, fireEvent } from '@testing-library/react';
import { createRef } from 'react';
import { CosmosGraph, type CosmosGraphHandle } from '../../src/internals/CosmosGraph.js';

/** Shared mock instance — reset per-test via beforeEach. */
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
};

let mockGraphInstance: MockGraph;
let mockGraphConstructor: Mock;
let mockGraphShouldThrow: Error | null = null;

vi.mock('@cosmos.gl/graph', () => {
  // Constructor mock — reads `mockGraphShouldThrow` to decide whether to
  // throw (simulating WebGL unavailability) or return `mockGraphInstance`.
  const Graph = vi.fn().mockImplementation(function (
    this: object,
    _container: HTMLElement,
    _config: unknown,
  ) {
    if (mockGraphShouldThrow) {
      throw mockGraphShouldThrow;
    }
    // Return the shared instance; the constructor's `this` binding is
    // irrelevant since we always replace with the stable mock object.
    Object.assign(this, mockGraphInstance);
    return mockGraphInstance;
  });
  return { Graph };
});

beforeEach(async () => {
  mockGraphShouldThrow = null;
  mockGraphInstance = {
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
  };
  const cosmosModule = (await import('@cosmos.gl/graph')) as unknown as {
    Graph: Mock;
  };
  mockGraphConstructor = cosmosModule.Graph;
  mockGraphConstructor.mockClear();
});

afterEach(() => {
  cleanup();
});

const minimalProps = () => ({
  config: {},
  pointPositions: new Float32Array([0, 0]),
  pointColors: new Float32Array([1, 0, 0, 1]),
  pointSizes: new Float32Array([4]),
});

describe('CosmosGraph', () => {
  it('mounts cosmos.gl Graph on initial render and destroys on unmount', () => {
    const { unmount } = render(<CosmosGraph {...minimalProps()} />);

    expect(mockGraphConstructor).toHaveBeenCalledTimes(1);
    // The container element + a config object containing the wrapper-injected
    // event callbacks.
    const [container, config] = mockGraphConstructor.mock.calls[0]!;
    expect(container).toBeInstanceOf(HTMLDivElement);
    expect(config).toMatchObject({
      onPointMouseOver: expect.any(Function),
      onPointMouseOut: expect.any(Function),
    });

    unmount();
    expect(mockGraphInstance.destroy).toHaveBeenCalledTimes(1);
  });

  it('renders the WebGL-not-available fallback when the Graph constructor throws', () => {
    mockGraphShouldThrow = new Error('webgl2 unavailable');

    render(<CosmosGraph {...minimalProps()} />);

    expect(screen.getByText(/WebGL not available/i)).toBeTruthy();
    const retryButton = screen.getByRole('button', { name: /retry/i });
    expect(retryButton).toBeTruthy();

    // Clicking Retry flips the error flag → component re-mounts the host div.
    // The mock still throws on the second mount attempt, so we just verify the
    // state-flip transition by re-allowing success and re-rendering.
    mockGraphShouldThrow = null;
    act(() => {
      fireEvent.click(retryButton);
    });

    // After Retry: host div should be present (no fallback).
    expect(screen.queryByText(/WebGL not available/i)).toBeNull();
    expect(document.querySelector('.fossil-viewer-cosmos-host')).toBeTruthy();
  });

  it('exposes the 8-method imperative handle and dispatches to the underlying Graph', () => {
    const ref = createRef<CosmosGraphHandle>();
    render(<CosmosGraph {...minimalProps()} ref={ref} />);

    expect(ref.current).not.toBeNull();
    expect(typeof ref.current!.zoomIn).toBe('function');
    expect(typeof ref.current!.zoomOut).toBe('function');
    expect(typeof ref.current!.fitView).toBe('function');
    expect(typeof ref.current!.start).toBe('function');
    expect(typeof ref.current!.pause).toBe('function');
    expect(typeof ref.current!.selectPointsByIndices).toBe('function');
    expect(typeof ref.current!.unselectPoints).toBe('function');
    // The `graph` getter exposes the underlying instance — non-null after mount.
    expect(ref.current!.graph).not.toBeNull();

    ref.current!.zoomIn(100);
    // getZoomLevel mocked to return 1 → expected zoom call: zoom(1.5, 100).
    expect(mockGraphInstance.zoom).toHaveBeenCalledWith(1.5, 100);

    ref.current!.fitView(500);
    expect(mockGraphInstance.fitView).toHaveBeenCalledWith(500);

    ref.current!.selectPointsByIndices([0, 1]);
    expect(mockGraphInstance.selectPointsByIndices).toHaveBeenCalledWith([0, 1]);

    ref.current!.unselectPoints();
    expect(mockGraphInstance.selectPointsByIndices).toHaveBeenLastCalledWith(
      null,
    );
  });
});
