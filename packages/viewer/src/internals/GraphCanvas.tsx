/**
 * GraphCanvas — composition layer of `@fossil-lang/viewer`.
 *
 * Wraps `CosmosGraph` (the lowest-level WebGL mount from plan 12-02) with:
 *  - adaptive simulation config (`getAdaptiveConfig(nodeCount)`)
 *  - data adapter (`useGraphData(vertices, edges)`)
 *  - crossfilter wiring (`useGraphCrossfilter(selection)`) — opt-in
 *  - bottom-left legend with one Toggle per vertex type (visibility +
 *    count badge + color dot)
 *  - pause-on-visibilitychange + keyboard shortcuts (`f` = fitView,
 *    Space = play/pause)
 *  - loader fallback when graphData is still being built
 *  - empty-state fallback when there are no visible vertices
 *
 * Port of Keasy `web/src/components/discovery/graph-view-v2.tsx` L80-201
 * (Phase 12 plan 12-03). Byte-equivalent behaviour modulo:
 *   - Data source: takes raw `VertexRow[]/EdgeRow[]` instead of a
 *     GraphSchema (Keasy reads from DuckDB).
 *   - CSS: Tailwind utility classes replaced by inline-style objects +
 *     `--fossil-*` CSS variables (per 12-03-PLAN.md `<css-mapping>`).
 *   - Icons: `lucide-react` `Loader2`/`Network` replaced by plain text
 *     ("Loading..." / "No data") — viewer is icon-library-free per
 *     CONTEXT.md.
 *   - `Badge` shadcn component replaced by a plain styled `<span>` —
 *     too small to vendor.
 *
 * @see Keasy keasy/web/src/components/discovery/graph-view-v2.tsx L80-201
 * @see .planning/phases/12-fossil-lang-viewer-port/12-03-PLAN.md
 */

import * as React from 'react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import type { GraphConfigInterface } from '@cosmos.gl/graph';
import type { Selection } from '@uwdata/mosaic-core';

import { Toggle } from '@fossil-lang/ui';

import { CosmosGraph, type CosmosGraphHandle } from './CosmosGraph.js';
import { getAdaptiveConfig } from './getAdaptiveConfig.js';
import {
  useGraphData,
  GROUP_CSS_COLORS,
  type VertexRow,
  type EdgeRow,
} from '../hooks/useGraphData.js';
import { useGraphCrossfilter } from '../hooks/useGraphCrossfilter.js';

export interface GraphCanvasProps {
  vertices: VertexRow[];
  edges: EdgeRow[];
  /** Optional override config merged ON TOP of the adaptive base. */
  graphConfig?: GraphConfigInterface;
  graphRef: React.RefObject<CosmosGraphHandle | null>;
  /** Mosaic Selection — crossfilter active only when present. */
  selection?: Selection | null;
  onSelectVertex?: (
    v: { id: string; type: string; label: string } | null,
  ) => void;
}

export function GraphCanvas({
  vertices,
  edges,
  graphConfig,
  graphRef,
  selection,
  onSelectVertex,
}: GraphCanvasProps): JSX.Element {
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [hiddenGroups, setHiddenGroups] = useState<Set<string>>(new Set());
  const [simulationRunning, setSimulationRunning] = useState(true);

  const graphData = useGraphData(vertices, edges);
  const { publishSelection, clearSelection } = useGraphCrossfilter(
    graphData,
    graphRef.current?.graph ?? null,
    selection,
  );

  // Pause on visibility change (Keasy L89-96).
  useEffect(() => {
    function handleVisibility(): void {
      if (document.hidden) graphRef.current?.pause();
      else graphRef.current?.start();
    }
    document.addEventListener('visibilitychange', handleVisibility);
    return () =>
      document.removeEventListener('visibilitychange', handleVisibility);
  }, [graphRef]);

  // Keyboard: F = fit, Space = play/pause (Keasy L99-107).
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent): void {
      if (
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement
      )
        return;
      if (e.key === 'f' && !e.metaKey && !e.ctrlKey) {
        e.preventDefault();
        graphRef.current?.fitView(500);
      }
      if (e.key === ' ' && !e.metaKey && !e.ctrlKey) {
        e.preventDefault();
        if (simulationRunning) graphRef.current?.pause();
        else graphRef.current?.start();
        setSimulationRunning((p) => !p);
      }
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [graphRef, simulationRunning]);

  // Config: adaptive base + user overrides + event handlers (Keasy L110-128).
  const nodeCount = graphData?.ids.length ?? 0;
  const config = useMemo<GraphConfigInterface>(
    () => ({
      ...getAdaptiveConfig(nodeCount),
      ...graphConfig,
      onClick: (index: number | undefined) => {
        setSelectedIndex(index ?? null);
        if (index != null && graphData) {
          publishSelection([index]);
          onSelectVertex?.({
            id: graphData.ids[index]!,
            type: graphData.types[index]!,
            label: graphData.labels[index]!,
          });
        } else {
          clearSelection();
          onSelectVertex?.(null);
        }
      },
      onSimulationEnd: () => {
        setSimulationRunning(false);
        graphRef.current?.fitView(500);
      },
    }),
    [
      graphConfig,
      nodeCount,
      graphData,
      publishSelection,
      clearSelection,
      onSelectVertex,
      graphRef,
    ],
  );

  // Group toggle (Keasy L131-133).
  const toggleGroup = useCallback((name: string) => {
    setHiddenGroups((prev) => {
      const n = new Set(prev);
      if (n.has(name)) n.delete(name);
      else n.add(name);
      return n;
    });
  }, []);

  // visibleData: alpha=0 + size=0 for vertices in hidden groups (Keasy L135-143).
  const visibleData = useMemo(() => {
    if (!graphData || hiddenGroups.size === 0) return graphData;
    const colors = new Float32Array(graphData.pointColors);
    const sizes = new Float32Array(graphData.pointSizes);
    for (let i = 0; i < graphData.types.length; i++) {
      if (hiddenGroups.has(graphData.types[i]!)) {
        colors[i * 4 + 3] = 0;
        sizes[i] = 0;
      }
    }
    return { ...graphData, pointColors: colors, pointSizes: sizes };
  }, [graphData, hiddenGroups]);

  // Group entries for legend (Keasy L145-152).
  const groupEntries = useMemo(() => {
    if (!graphData) return [];
    const counts = new Map<string, number>();
    for (const t of graphData.types) counts.set(t, (counts.get(t) ?? 0) + 1);
    return [...counts.entries()].map(([name, count], i) => ({
      name,
      count,
      color: GROUP_CSS_COLORS[i % GROUP_CSS_COLORS.length]!,
    }));
  }, [graphData]);

  // Loader fallback — graphData still being built (Keasy L154-159 / mapping #1).
  if (graphData === null) {
    return (
      <div
        className="fossil-viewer-loader"
        style={{
          position: 'absolute',
          inset: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <span
          className="fossil-viewer-loader-text"
          style={{
            fontSize: 'var(--fossil-fonts-sizeXs, 11px)',
            color: 'var(--fossil-colors-mutedForeground, #475569)',
          }}
        >
          Loading...
        </span>
      </div>
    );
  }

  // Empty-state fallback (Keasy L162-167 / mapping #2).
  if (!visibleData || visibleData.ids.length === 0) {
    return (
      <div
        className="fossil-viewer-empty-state"
        style={{
          position: 'absolute',
          inset: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <span
          style={{
            fontSize: 'var(--fossil-fonts-sizeSmall, 13px)',
            color: 'var(--fossil-colors-mutedForeground, #475569)',
          }}
        >
          No data
        </span>
      </div>
    );
  }

  // Main render (Keasy L170-200 / mappings #3-#8).
  return (
    <div
      className="fossil-viewer-canvas-root"
      style={{ position: 'absolute', inset: 0 }}
    >
      <CosmosGraph
        ref={graphRef as React.Ref<CosmosGraphHandle>}
        config={config}
        pointPositions={visibleData.pointPositions}
        pointColors={visibleData.pointColors}
        pointSizes={visibleData.pointSizes}
        linkIndexes={visibleData.linkIndexes}
        pointClusters={visibleData.pointClusters}
        clusterPositions={visibleData.clusterPositions}
        focusedPointIndex={selectedIndex ?? undefined}
        renderPointTooltip={(i) => (
          <div
            style={{ fontSize: 'var(--fossil-fonts-sizeXs, 11px)' }}
          >
            <p style={{ margin: 0, fontWeight: 500 }}>
              {visibleData.labels[i]}
            </p>
            <p
              style={{
                margin: 0,
                color: 'var(--fossil-colors-mutedForeground, #475569)',
              }}
            >
              {visibleData.types[i]}
            </p>
          </div>
        )}
      />

      {/* Legend — bottom left (only when > 1 unique type). */}
      {groupEntries.length > 1 && (
        <div
          className="fossil-viewer-legend"
          style={{
            position: 'absolute',
            bottom: '0.75rem',
            left: '0.75rem',
            background:
              'var(--fossil-colors-card, rgba(255, 255, 255, 0.9))',
            backdropFilter: 'blur(4px)',
            border:
              '1px solid var(--fossil-colors-border, #e2e8f0)',
            borderRadius: 'var(--fossil-radii-sm, 4px)',
            padding: '2px',
            fontSize: 'var(--fossil-fonts-sizeXs, 11px)',
            userSelect: 'none',
            zIndex: 10,
          }}
        >
          {groupEntries.map(({ name, count, color }) => (
            <Toggle
              key={name}
              pressed={!hiddenGroups.has(name)}
              onPressedChange={() => toggleGroup(name)}
              className="fossil-viewer-legend-toggle"
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '0.375rem',
                width: '100%',
                justifyContent: 'flex-start',
                height: '1.25rem',
                padding: '0 0.375rem',
                fontSize: 'var(--fossil-fonts-sizeXs, 11px)',
                borderRadius: 'var(--fossil-radii-sm, 4px)',
              }}
            >
              <span
                className="fossil-viewer-legend-dot"
                style={{
                  width: '8px',
                  height: '8px',
                  borderRadius: '9999px',
                  flexShrink: 0,
                  backgroundColor: color,
                  display: 'inline-block',
                }}
              />
              <span
                className="fossil-viewer-legend-label"
                style={{
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {name}
              </span>
              <span
                className="fossil-viewer-legend-count"
                style={{
                  marginLeft: 'auto',
                  fontSize: '9px',
                  padding: '0 0.25rem',
                  lineHeight: 1.1,
                  height: '0.875rem',
                  display: 'inline-flex',
                  alignItems: 'center',
                }}
              >
                {count.toLocaleString()}
              </span>
            </Toggle>
          ))}
        </div>
      )}
    </div>
  );
}
