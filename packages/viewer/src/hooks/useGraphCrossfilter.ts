/**
 * useGraphCrossfilter — bridges cosmos.gl graph ↔ Mosaic crossfilter.
 *
 * Port of Keasy `web/src/components/discovery/use-graph-crossfilter.ts`
 * L1-122 (Phase 12 plan 12-03). Pattern: a `MosaicClient` registered
 * with the global coordinator that receives widget filters → queries
 * DuckDB for matching `_id` → `selectPointsByIndices` (GPU grey-out);
 * and PUBLISHES graph selections back via `clausePoints` so other
 * Mosaic-aware widgets re-filter accordingly.
 *
 * Two divergences from Keasy:
 *
 *   1. Keasy reads `coordinator` from `useDiscoveryStore`. We use the
 *      `@uwdata/mosaic-core` global `coordinator()` accessor — when
 *      hosts wire their own coordinator, the registration here joins
 *      that one automatically (Mosaic's singleton pattern).
 *
 *   2. `selection` accepts `null | undefined` (the plan's REVISION to
 *      Task 1). When absent, both callbacks become no-ops and the
 *      makeClient effect skips registration. This lets `<FossilGraphView/>`
 *      (12-04) make `selection` an optional prop without breaking
 *      this hook's call signature when the prop is omitted.
 *
 * Production note: the query function presumes DuckDB-side views named
 * after the vertex types and carrying an `_id` column — a Keasy
 * convention. Hosts not satisfying that contract should leave
 * `selection` undefined; standalone viewer usage works without
 * crossfilter (clicking a vertex still invokes the
 * `onSelectVertex` callback in `GraphCanvas` regardless).
 *
 * @see Keasy keasy/web/src/components/discovery/use-graph-crossfilter.ts L1-122
 * @see .planning/phases/12-fossil-lang-viewer-port/12-03-PLAN.md (Task 1 REVISION)
 */

import { useCallback, useEffect, useRef } from 'react';
import {
  type Selection,
  coordinator as getCoordinator,
  makeClient,
  clausePoints,
} from '@uwdata/mosaic-core';
import { Query, column } from '@uwdata/mosaic-sql';
import type { Graph } from '@cosmos.gl/graph';
import type { FilterExpr } from '@uwdata/mosaic-sql';

import type { KGGraphData } from './useGraphData.js';

// Stable source identity for the graph's clauses.
const GRAPH_SOURCE = { reset: () => {} };

interface GraphCrossfilterResult {
  /** Publish a graph selection to the crossfilter (graph → widgets). */
  publishSelection: (denseIndices: number[]) => void;
  /** Clear the graph's selection from the crossfilter. */
  clearSelection: () => void;
}

/**
 * Connect a cosmos.gl Graph to a Mosaic crossfilter Selection.
 *
 * @param graphData - The current graph data (for index↔_id mapping).
 *   When null, both callbacks are no-ops.
 * @param graph - The cosmos.gl Graph instance. May be null before mount.
 * @param selection - The shared crossfilter Selection. When null or
 *   undefined, the hook short-circuits and returns no-op callbacks
 *   (per Task 1 REVISION in 12-03-PLAN.md).
 */
export function useGraphCrossfilter(
  graphData: KGGraphData | null,
  graph: Graph | null,
  selection: Selection | null | undefined,
): GraphCrossfilterResult {
  const clientRef = useRef<ReturnType<typeof makeClient> | null>(null);

  // Connect a MosaicClient that receives widget filters and highlights the graph.
  useEffect(() => {
    if (!selection || !graphData || !graph) return;

    const coordinator = getCoordinator();
    if (!coordinator) return;

    const types = [...new Set(graphData.types)];

    const client = makeClient({
      coordinator,
      selection,
      filterStable: true,
      query: (filter: FilterExpr | undefined) => {
        // When no active filter, return null (show all).
        if (!filter) return null;
        // Query all vertex types for `_id` matching the crossfilter predicate.
        const subqueries = types.map((t) =>
          Query.from(t).select('_id').where(filter).toString(),
        );
        return subqueries.join(' UNION ALL ');
      },
      queryResult: (data: unknown) => {
        if (!graph || !graphData) return;
        // Extract `_id` values from the result and map to dense indices.
        const rows = data as { _id: number }[] | null | undefined;
        if (!rows || !Array.isArray(rows)) {
          graph.unselectPoints();
          return;
        }
        const indices: number[] = [];
        for (const row of rows) {
          const dense = graphData.idToDense.get(row._id);
          if (dense !== undefined) indices.push(dense);
        }
        graph.selectPointsByIndices(indices);
      },
    });

    clientRef.current = client;
    return () => {
      coordinator.disconnect(client);
      clientRef.current = null;
    };
  }, [graphData, graph, selection]);

  // Publish graph selection → crossfilter (graph → widgets).
  const publishSelection = useCallback(
    (denseIndices: number[]) => {
      if (!graphData || !selection) return;
      if (denseIndices.length === 0) {
        selection.update(
          clausePoints([column('_id')], undefined, { source: GRAPH_SOURCE }),
        );
        return;
      }
      const ids = denseIndices.map((i) => graphData.denseToId[i]);
      selection.update(
        clausePoints(
          [column('_id')],
          ids.map((id) => [id]),
          { source: GRAPH_SOURCE },
        ),
      );
    },
    [graphData, selection],
  );

  // Clear the graph's own clause from the crossfilter.
  const clearSelection = useCallback(() => {
    if (!selection) return;
    graph?.unselectPoints();
    selection.update(
      clausePoints([column('_id')], undefined, { source: GRAPH_SOURCE }),
    );
  }, [graph, selection]);

  return { publishSelection, clearSelection };
}
