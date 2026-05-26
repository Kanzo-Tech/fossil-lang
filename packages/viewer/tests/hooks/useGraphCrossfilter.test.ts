/**
 * Unit tests for useGraphCrossfilter — Phase 12 plan 12-03.
 *
 * Mocks `@uwdata/mosaic-core` so the hook's behaviour can be exercised
 * without a real Mosaic coordinator. Verifies:
 *  1. publishSelection(denseIndices) calls selection.update with
 *     clausePoints(GRAPH_SOURCE).
 *  2. clearSelection() calls graph.unselectPoints + selection.update
 *     with `undefined` values.
 *  3. When selection is null/undefined, both callbacks are no-ops
 *     (the Task 1 REVISION in 12-03-PLAN.md).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import type { Graph } from '@cosmos.gl/graph';
import type { Selection } from '@uwdata/mosaic-core';
import type { KGGraphData } from '../../src/hooks/useGraphData.js';

const mockClausePoints = vi.fn((_cols, values, opts) => ({
  __clausePoints: true,
  values,
  opts,
}));
const mockCoordinator = {
  disconnect: vi.fn(),
};

vi.mock('@uwdata/mosaic-core', () => ({
  coordinator: vi.fn(() => mockCoordinator),
  makeClient: vi.fn(() => ({ __client: true })),
  clausePoints: (...args: unknown[]) =>
    mockClausePoints(...(args as Parameters<typeof mockClausePoints>)),
}));

vi.mock('@uwdata/mosaic-sql', () => ({
  Query: {
    from: vi.fn(() => ({
      select: vi.fn(() => ({
        where: vi.fn(() => ({ toString: () => 'SELECT _id FROM t' })),
      })),
    })),
  },
  column: vi.fn((name: string) => ({ __column: name })),
}));

import { useGraphCrossfilter } from '../../src/hooks/useGraphCrossfilter.js';

function makeGraphData(): KGGraphData {
  return {
    ids: ['a', 'b'],
    labels: ['Alice', 'Bob'],
    types: ['Person', 'Person'],
    denseToId: [0, 1],
    idToDense: new Map([
      [0, 0],
      [1, 1],
    ]),
    pointPositions: new Float32Array([0, 0, 1, 1]),
    pointColors: new Float32Array([1, 0, 0, 1, 0, 1, 0, 1]),
    pointSizes: new Float32Array([4, 4]),
    linkIndexes: new Float32Array([0, 1]),
    pointClusters: [0, 0],
    clusterPositions: [0, 0],
  };
}

function makeSelection(): Selection {
  return { update: vi.fn() } as unknown as Selection;
}

function makeGraph(): Graph {
  return {
    unselectPoints: vi.fn(),
    selectPointsByIndices: vi.fn(),
  } as unknown as Graph;
}

beforeEach(() => {
  mockClausePoints.mockClear();
  mockCoordinator.disconnect.mockClear();
});

describe('useGraphCrossfilter', () => {
  it('publishSelection emits clausePoints carrying GRAPH_SOURCE source identity', () => {
    const graphData = makeGraphData();
    const graph = makeGraph();
    const selection = makeSelection();

    const { result } = renderHook(() =>
      useGraphCrossfilter(graphData, graph, selection),
    );

    result.current.publishSelection([0]);

    expect(selection.update).toHaveBeenCalledTimes(1);
    expect(mockClausePoints).toHaveBeenCalledTimes(1);
    const [, values, opts] = mockClausePoints.mock.calls[0]!;
    expect(values).toEqual([[0]]); // denseToId[0] = 0
    expect(opts).toMatchObject({ source: expect.any(Object) });
  });

  it('clearSelection calls graph.unselectPoints + selection.update with undefined values', () => {
    const graphData = makeGraphData();
    const graph = makeGraph();
    const selection = makeSelection();

    const { result } = renderHook(() =>
      useGraphCrossfilter(graphData, graph, selection),
    );

    result.current.clearSelection();

    expect(graph.unselectPoints).toHaveBeenCalledTimes(1);
    expect(selection.update).toHaveBeenCalledTimes(1);
    const [, values] = mockClausePoints.mock.calls[0]!;
    expect(values).toBeUndefined();
  });

  it('returns no-op callbacks when selection is null (Task 1 REVISION)', () => {
    const graphData = makeGraphData();
    const graph = makeGraph();

    const { result } = renderHook(() =>
      useGraphCrossfilter(graphData, graph, null),
    );

    // No throws.
    result.current.publishSelection([0]);
    result.current.clearSelection();

    expect(mockClausePoints).not.toHaveBeenCalled();
    expect(graph.unselectPoints).not.toHaveBeenCalled();
  });
});
