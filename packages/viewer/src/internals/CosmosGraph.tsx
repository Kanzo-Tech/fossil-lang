/**
 * CosmosGraph — React wrapper for `@cosmos.gl/graph`.
 *
 * `@cosmos.gl/graph` creates a WebGL canvas inside a container div and
 * manages its own ResizeObserver. This component handles lifecycle
 * (create / destroy), data updates, tooltip rendering, and exposes the
 * `Graph` instance via ref.
 *
 * Phase 12 plan 12-02 — port of Keasy `web/src/components/discovery/
 * cosmos-graph.tsx` (171 lines). Byte-equivalent semantics modulo the
 * Tailwind-utility-class to inline-style + `--fossil-*` CSS-variable
 * mapping documented in 12-02-PLAN.md `<css-mapping>`. Every behavioural
 * divergence from Keasy is a defect.
 *
 * Notes vs Keasy:
 *  - The `"use client"` Next.js directive is intentionally removed —
 *    `@fossil-lang/viewer` ships pure ESM and is consumed by hosts that
 *    handle their own SSR boundaries.
 *  - The `:active { cursor: grabbing; }` pseudo-state for the host div
 *    is omitted (Tailwind ships it via `active:cursor-grabbing`; we have
 *    no static stylesheet here). Hosts can layer
 *    `.fossil-viewer-cosmos-host:active { cursor: grabbing; }` if they
 *    want the affordance — purely cosmetic.
 *  - Each visible element gets a `fossil-viewer-cosmos-*` className so
 *    hosts can override styles without selector gymnastics.
 *
 * @see .planning/phases/12-fossil-lang-viewer-port/12-02-PLAN.md
 * @see .planning/phases/12-fossil-lang-viewer-port/12-CONTEXT.md
 */

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { Graph, type GraphConfigInterface } from '@cosmos.gl/graph';
import { createPortal } from 'react-dom';

export interface CosmosGraphProps {
  config: GraphConfigInterface;
  pointPositions: Float32Array;
  pointColors: Float32Array;
  pointSizes: Float32Array;
  linkIndexes?: Float32Array;
  pointClusters?: (number | undefined)[];
  clusterPositions?: (number | undefined)[];
  focusedPointIndex?: number;
  renderPointTooltip?: (index: number) => ReactNode;
}

/** Imperative handle exposing `@cosmos.gl/graph` Graph methods to parents. */
export interface CosmosGraphHandle {
  graph: Graph | null;
  zoomIn: (duration?: number) => void;
  zoomOut: (duration?: number) => void;
  fitView: (duration?: number) => void;
  start: () => void;
  pause: () => void;
  selectPointsByIndices: (indices: (number | undefined)[]) => void;
  unselectPoints: () => void;
}

export const CosmosGraph = forwardRef<CosmosGraphHandle, CosmosGraphProps>(
  function CosmosGraph(props, ref) {
    const containerRef = useRef<HTMLDivElement>(null);
    const graphRef = useRef<Graph | null>(null);
    const [tooltip, setTooltip] = useState<{
      index: number;
      x: number;
      y: number;
    } | null>(null);
    const [webglError, setWebglError] = useState(false);

    // Expose typed handle to parent.
    useImperativeHandle(
      ref,
      () => ({
        get graph() {
          return graphRef.current;
        },
        zoomIn: (duration = 300) => {
          const g = graphRef.current;
          if (g) g.zoom(g.getZoomLevel() * 1.5, duration);
        },
        zoomOut: (duration = 300) => {
          const g = graphRef.current;
          if (g) g.zoom(g.getZoomLevel() / 1.5, duration);
        },
        fitView: (duration?: number) => graphRef.current?.fitView(duration),
        start: () => graphRef.current?.start(),
        pause: () => graphRef.current?.pause(),
        selectPointsByIndices: (indices: (number | undefined)[]) =>
          graphRef.current?.selectPointsByIndices(indices),
        unselectPoints: () => graphRef.current?.selectPointsByIndices(null),
      }),
      [],
    );

    // Create graph on mount.
    useEffect(() => {
      if (!containerRef.current) return;
      let graph: Graph;
      try {
        graph = new Graph(containerRef.current, {
          ...props.config,
          onPointMouseOver: (
            index: number,
            _pos: [number, number],
            event: unknown,
          ) => {
            const e = event as MouseEvent | undefined;
            if (props.renderPointTooltip && index !== undefined && e) {
              setTooltip({
                index,
                x: e.clientX,
                y: e.clientY,
              });
            }
          },
          onPointMouseOut: () => setTooltip(null),
        });
        graphRef.current = graph;
      } catch {
        setWebglError(true);
        return;
      }
      return () => {
        graph.destroy();
        graphRef.current = null;
      };
      // Only run on mount/unmount — config changes handled by setConfig below.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // Update config (simulation, visual, events).
    useEffect(() => {
      graphRef.current?.setConfig(props.config);
    }, [props.config]);

    // Update point data.
    useEffect(() => {
      const g = graphRef.current;
      if (!g) return;
      g.setPointPositions(props.pointPositions);
      g.setPointColors(props.pointColors);
      g.setPointSizes(props.pointSizes);
      if (props.linkIndexes) g.setLinks(props.linkIndexes);
      if (props.pointClusters) g.setPointClusters(props.pointClusters);
      if (props.clusterPositions)
        g.setClusterPositions(props.clusterPositions);
      g.render();
    }, [
      props.pointPositions,
      props.pointColors,
      props.pointSizes,
      props.linkIndexes,
      props.pointClusters,
      props.clusterPositions,
    ]);

    // Focused point.
    useEffect(() => {
      graphRef.current?.setConfig({
        focusedPointIndex: props.focusedPointIndex,
      });
    }, [props.focusedPointIndex]);

    if (webglError) {
      return (
        <div
          className="fossil-viewer-cosmos-error"
          style={{
            width: '100%',
            height: '100%',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          <div
            style={{
              textAlign: 'center',
              display: 'flex',
              flexDirection: 'column',
              gap: '0.5rem',
            }}
          >
            <p
              className="fossil-viewer-cosmos-error-title"
              style={{
                fontSize: 'var(--fossil-fonts-sizeSmall, 13px)',
                color: 'var(--fossil-colors-destructive, #ef4444)',
                fontWeight: 500,
                margin: 0,
              }}
            >
              WebGL not available
            </p>
            <p
              className="fossil-viewer-cosmos-error-detail"
              style={{
                fontSize: 'var(--fossil-fonts-sizeXs, 11px)',
                color: 'var(--fossil-colors-mutedForeground, #475569)',
                margin: 0,
              }}
            >
              Your browser or GPU does not support WebGL, which is required for
              graph visualization.
            </p>
            <button
              type="button"
              className="fossil-viewer-cosmos-retry"
              style={{
                fontSize: 'var(--fossil-fonts-sizeXs, 11px)',
                color: 'var(--fossil-colors-primary, #0f172a)',
                textDecoration: 'underline',
                background: 'none',
                border: 'none',
                cursor: 'pointer',
                padding: 0,
              }}
              onClick={() => {
                setWebglError(false);
              }}
            >
              Retry
            </button>
          </div>
        </div>
      );
    }

    return (
      <>
        <div
          ref={containerRef}
          className="fossil-viewer-cosmos-host"
          style={{
            width: '100%',
            height: '100%',
            cursor: 'grab',
          }}
        />
        {tooltip &&
          props.renderPointTooltip &&
          createPortal(
            <div
              className="fossil-viewer-cosmos-tooltip"
              style={{
                pointerEvents: 'none',
                position: 'fixed',
                zIndex: 50,
                left: tooltip.x + 12,
                top: tooltip.y + 12,
                borderRadius: 'var(--fossil-radii-md, 6px)',
                border: '1px solid var(--fossil-colors-border, #e2e8f0)',
                background: 'var(--fossil-colors-popover, #ffffff)',
                padding: '0.25rem 0.5rem',
                boxShadow: '0 4px 6px -1px rgba(0, 0, 0, 0.1)',
              }}
            >
              {props.renderPointTooltip(tooltip.index)}
            </div>,
            document.body,
          )}
      </>
    );
  },
);
