/**
 * useGraphData — adapter hook turning the public VertexRow[]/EdgeRow[]
 * input shape into the cosmos.gl-ready KGGraphData typed arrays.
 *
 * Port of Keasy `web/src/components/discovery/use-graph-data.ts` L20-203
 * (Phase 12 plan 12-03) SALVO the data source: Keasy reads from a
 * DuckDB query coordinator via `useCoordinatorQuery`; this hook accepts
 * raw vertex/edge arrays so `@fossil-lang/viewer` is decoupled from the
 * Keasy discovery store + DuckDB runtime. The transformation logic
 * (Float32Array building, cluster layout, color palette, hash-seeded
 * positions) is byte-equivalent.
 *
 * Key divergence from Keasy: vertex/edge ids are STRINGS in this
 * public API (Keasy uses numeric GraphAr `_id`). The internal
 * `denseToId` / `idToDense` mapping therefore reuses the dense index
 * itself as the numeric id surrogate; downstream consumers needing the
 * original string id index into `ids[denseIndex]`.
 *
 * @see Keasy keasy/web/src/components/discovery/use-graph-data.ts L20-203
 * @see .planning/phases/12-fossil-lang-viewer-port/12-CONTEXT.md
 */

import { useMemo } from 'react';

// ── Public input shapes (CONTEXT.md locked) ──────────────────────────────

export interface VertexRow {
  id: string;
  type: string;
  label: string;
  [k: string]: unknown;
}

export interface EdgeRow {
  source: string;
  target: string;
  predicate?: string;
  [k: string]: unknown;
}

// ── Output shape (cosmos.gl-ready typed arrays) ──────────────────────────

export interface KGGraphData {
  /** Original string ids, indexed by dense index. */
  ids: string[];
  labels: string[];
  types: string[];
  /** Dense index → numeric id surrogate (we use dense index itself; see file header). */
  denseToId: number[];
  /** Numeric id surrogate → dense index. */
  idToDense: Map<number, number>;
  /** cosmos.gl typed arrays (positions [x0,y0,x1,y1,...], colors [r,g,b,a,...], sizes [s0,s1,...]). */
  pointPositions: Float32Array;
  pointColors: Float32Array;
  pointSizes: Float32Array;
  /** Edge endpoints as dense indices [src0,tgt0,src1,tgt1,...]. */
  linkIndexes: Float32Array;
  /** Cluster index per vertex (grouped by type). */
  pointClusters: (number | undefined)[];
  /** Cluster center positions [x0,y0,x1,y1,...] distributed in circle. */
  clusterPositions: (number | undefined)[];
}

// ── Color palette (verbatim Keasy use-graph-data.ts L43-51) ──────────────

const COLORS: [number, number, number][] = [
  [59, 130, 246],
  [34, 197, 94],
  [168, 85, 247],
  [249, 115, 22],
  [239, 68, 68],
  [20, 184, 166],
  [234, 179, 8],
  [236, 72, 153],
];

export const GROUP_CSS_COLORS = [
  '#3b82f6',
  '#22c55e',
  '#a855f7',
  '#f97316',
  '#ef4444',
  '#14b8a6',
  '#eab308',
  '#ec4899',
];

// ── Position seed (deterministic, stable across re-renders) ──────────────

/** Hash a string id to a deterministic 2D position offset. */
export function hashPos(id: string): [number, number] {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = ((h << 5) - h + id.charCodeAt(i)) | 0;
  return [
    ((h & 0xffff) / 0xffff - 0.5) * 1000,
    (((h >>> 16) & 0xffff) / 0xffff - 0.5) * 1000,
  ];
}

// ── Hook ─────────────────────────────────────────────────────────────────

/**
 * Build the cosmos.gl-ready KGGraphData from raw vertex/edge rows.
 *
 * Returns `null` for the empty-input case so consumers can render a
 * loader/empty-state at the call site (preserves Keasy's null sentinel).
 */
export function useGraphData(
  vertices: VertexRow[],
  edges: EdgeRow[],
): KGGraphData | null {
  return useMemo(() => {
    if (vertices.length === 0) return null;

    const n = vertices.length;
    const ids: string[] = [];
    const labels: string[] = [];
    const types: string[] = [];
    const denseToId: number[] = [];
    const idToDense = new Map<number, number>();
    /** String-id → dense-index lookup for edge resolution (Keasy uses _id; we use string id). */
    const stringIdToDense = new Map<string, number>();
    const groupColorIdx = new Map<string, number>();

    for (let i = 0; i < n; i++) {
      const row = vertices[i]!;
      ids.push(row.id);
      labels.push(row.label ?? row.id);
      types.push(row.type);
      denseToId.push(i);
      idToDense.set(i, i);
      stringIdToDense.set(row.id, i);
      if (!groupColorIdx.has(row.type)) {
        groupColorIdx.set(row.type, groupColorIdx.size % COLORS.length);
      }
    }

    // Cluster assignments: each type → a cluster index.
    const clusterMap = new Map<string, number>();
    for (const t of types) {
      if (!clusterMap.has(t)) clusterMap.set(t, clusterMap.size);
    }
    const pointClusters: (number | undefined)[] = types.map((t) =>
      clusterMap.get(t),
    );

    // Cluster centers distributed in circle, radius scales with sqrt(n).
    const numClusters = clusterMap.size;
    const clusterPositions: (number | undefined)[] = [];
    const radius = Math.sqrt(n) * 3;
    for (let ci = 0; ci < numClusters; ci++) {
      const angle = (2 * Math.PI * ci) / numClusters;
      clusterPositions.push(Math.cos(angle) * radius, Math.sin(angle) * radius);
    }

    // Positions: seed near cluster center with small hash-based offset.
    const positions = new Float32Array(n * 2);
    const colors = new Float32Array(n * 4);
    const sizes = new Float32Array(n).fill(4);

    for (let i = 0; i < n; i++) {
      const type = types[i]!;
      const ci = clusterMap.get(type) ?? 0;
      const cx = clusterPositions[ci * 2] as number;
      const cy = clusterPositions[ci * 2 + 1] as number;
      const [hx, hy] = hashPos(ids[i]!);
      // Small offset from cluster center — simulation refines from here.
      positions[i * 2] = cx + hx * 0.05;
      positions[i * 2 + 1] = cy + hy * 0.05;

      const colorIdx = groupColorIdx.get(type) ?? 0;
      const [r, g, b] = COLORS[colorIdx]!;
      colors[i * 4] = r / 255;
      colors[i * 4 + 1] = g / 255;
      colors[i * 4 + 2] = b / 255;
      colors[i * 4 + 3] = 1;
    }

    // Edges: pre-allocate, resolve string-ids to dense indices, then trim.
    const edgeLen = edges.length;
    const linkBuf = new Float32Array(edgeLen * 2);
    let edgeIdx = 0;
    for (let i = 0; i < edgeLen; i++) {
      const row = edges[i]!;
      const si = stringIdToDense.get(row.source);
      const ti = stringIdToDense.get(row.target);
      if (si !== undefined && ti !== undefined) {
        linkBuf[edgeIdx++] = si;
        linkBuf[edgeIdx++] = ti;
      }
    }

    return {
      ids,
      labels,
      types,
      denseToId,
      idToDense,
      pointPositions: positions,
      pointColors: colors,
      pointSizes: sizes,
      linkIndexes:
        edgeIdx < linkBuf.length ? linkBuf.subarray(0, edgeIdx) : linkBuf,
      pointClusters,
      clusterPositions,
    };
  }, [vertices, edges]);
}
