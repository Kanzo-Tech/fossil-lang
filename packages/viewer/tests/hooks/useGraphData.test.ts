/**
 * Unit tests for useGraphData — Phase 12 plan 12-03.
 *
 * Verifies the data-adapter shape contract (KGGraphData typed arrays)
 * and the string-id edge-resolution divergence from Keasy (we use
 * `stringIdToDense` because the public VertexRow API is string-keyed,
 * whereas Keasy resolves via numeric GraphAr `_id`).
 */
import { describe, it, expect } from 'vitest';
import { renderHook } from '@testing-library/react';
import {
  useGraphData,
  type VertexRow,
  type EdgeRow,
} from '../../src/hooks/useGraphData.js';

describe('useGraphData', () => {
  it('returns null for empty input (Keasy null-sentinel preserved)', () => {
    const { result } = renderHook(() => useGraphData([], []));
    expect(result.current).toBeNull();
  });

  it('produces correct typed-array shapes for 3 vertices of 2 types', () => {
    const vertices: VertexRow[] = [
      { id: 'a', type: 'Person', label: 'Alice' },
      { id: 'b', type: 'Person', label: 'Bob' },
      { id: 'c', type: 'Org', label: 'Acme' },
    ];
    const { result } = renderHook(() => useGraphData(vertices, []));

    expect(result.current).not.toBeNull();
    const data = result.current!;
    expect(data.ids).toEqual(['a', 'b', 'c']);
    expect(data.labels).toEqual(['Alice', 'Bob', 'Acme']);
    expect(data.types).toEqual(['Person', 'Person', 'Org']);
    expect(data.pointPositions).toBeInstanceOf(Float32Array);
    expect(data.pointPositions.length).toBe(6); // 3 * 2
    expect(data.pointColors).toBeInstanceOf(Float32Array);
    expect(data.pointColors.length).toBe(12); // 3 * 4
    expect(data.pointSizes).toBeInstanceOf(Float32Array);
    expect(data.pointSizes.length).toBe(3);
    // Two unique cluster indices (one per type).
    const uniqueClusters = new Set(data.pointClusters);
    expect(uniqueClusters.size).toBe(2);
  });

  it('resolves string-id edges via stringIdToDense lookup', () => {
    const vertices: VertexRow[] = [
      { id: 'a', type: 'Person', label: 'Alice' },
      { id: 'b', type: 'Person', label: 'Bob' },
    ];
    const edges: EdgeRow[] = [{ source: 'a', target: 'b', predicate: 'knows' }];
    const { result } = renderHook(() => useGraphData(vertices, edges));

    const data = result.current!;
    expect(data.linkIndexes.length).toBe(2);
    // a → b maps to dense 0 → dense 1.
    expect(data.linkIndexes[0]).toBe(0);
    expect(data.linkIndexes[1]).toBe(1);
  });

  it('silently skips edges with unknown endpoints', () => {
    const vertices: VertexRow[] = [
      { id: 'a', type: 'Person', label: 'Alice' },
    ];
    const edges: EdgeRow[] = [
      { source: 'a', target: 'nonexistent' },
      { source: 'doesnt-exist', target: 'a' },
    ];
    const { result } = renderHook(() => useGraphData(vertices, edges));

    const data = result.current!;
    // Both edges have one unresolvable endpoint → linkIndexes is empty.
    expect(data.linkIndexes.length).toBe(0);
  });
});
