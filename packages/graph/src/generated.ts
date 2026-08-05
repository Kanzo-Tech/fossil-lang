/* eslint-disable */
/**
 * GENERATED — do not edit by hand.
 * Source: crates/fossil-graph JSON Schemas (schemars). Regenerate with
 *   pnpm --filter @fossil-lang/graph gen:types
 */

export type Aggregation = "count" | "sum" | "avg" | "min" | "max";
export type FieldRole = "identifier" | "dimension" | "measure";
export type HistogramKind = "numeric" | "temporal" | "categorical";
/**
 * All graph operations dispatchable on the surface.
 *
 * The `tag = "verb"` serde representation makes the wire form `{ "verb": "list_vertex_types", "params": { … } }` — identical for MCP tool calls, HTTP POST bodies, and CLI subcommand args.
 */
export type Operation =
  | {
      params: ListVertexTypesParams;
      verb: "list_vertex_types";
    }
  | {
      params: ListEdgeTypesParams;
      verb: "list_edge_types";
    }
  | {
      params: DescribeFieldParams;
      verb: "describe_field";
    }
  | {
      params: DescribeVertexTypeParams;
      verb: "describe_vertex_type";
    }
  | {
      params: FindNeighborsParams;
      verb: "find_neighbors";
    }
  | {
      params: FindPathParams;
      verb: "find_path";
    }
  | {
      params: GetVertexParams;
      verb: "get_vertex";
    }
  | {
      params: AggregateParams;
      verb: "aggregate";
    }
  | {
      params: HistogramParams;
      verb: "histogram";
    }
  | {
      params: TopKParams;
      verb: "top_k";
    }
  | {
      params: ViewportParams;
      verb: "viewport";
    }
  | {
      params: MaterializeGraphParams;
      verb: "materialize_graph";
    }
  | {
      params: ExecuteSqlParams;
      verb: "execute_sql";
    };
export type ViewportMode = "detail" | "aggregate";

export interface FossilGraphSchemas {
  AggregateParams?: AggregateParams;
  AggregateResult?: AggregateResult;
  AggregateRow?: AggregateRow;
  Aggregation?: Aggregation;
  BoundingBox?: BoundingBox;
  ColumnDescriptor?: ColumnDescriptor;
  DescribeFieldParams?: DescribeFieldParams;
  DescribeFieldResult?: DescribeFieldResult;
  DescribeVertexTypeParams?: DescribeVertexTypeParams;
  DescribeVertexTypeResult?: DescribeVertexTypeResult;
  EdgeTypeSummary?: EdgeTypeSummary;
  ExecuteSqlParams?: ExecuteSqlParams;
  ExecuteSqlResult?: ExecuteSqlResult;
  FieldRole?: FieldRole;
  FieldStat?: FieldStat;
  FindNeighborsParams?: FindNeighborsParams;
  FindNeighborsResult?: FindNeighborsResult;
  FindPathParams?: FindPathParams;
  FindPathResult?: FindPathResult;
  GetVertexParams?: GetVertexParams;
  GetVertexResult?: GetVertexResult;
  HistogramKind?: HistogramKind;
  HistogramParams?: HistogramParams;
  HistogramResult?: HistogramResult;
  ListEdgeTypesParams?: ListEdgeTypesParams;
  ListEdgeTypesResult?: ListEdgeTypesResult;
  ListVertexTypesParams?: ListVertexTypesParams;
  ListVertexTypesResult?: ListVertexTypesResult;
  MaterializeGraphParams?: MaterializeGraphParams;
  MaterializeGraphResult?: MaterializeGraphResult;
  MaterializedEdge?: MaterializedEdge;
  MaterializedVertex?: MaterializedVertex;
  NeighborEdge?: NeighborEdge;
  NeighborVertex?: NeighborVertex;
  Operation?: Operation;
  TopKParams?: TopKParams;
  TopKResult?: TopKResult;
  VertexTypeSummary?: VertexTypeSummary;
  ViewportEdge?: ViewportEdge;
  ViewportMode?: ViewportMode;
  ViewportParams?: ViewportParams;
  ViewportResult?: ViewportResult;
  ViewportVertex?: ViewportVertex;
}
export interface AggregateParams {
  agg: Aggregation;
  group_by: string;
  limit?: number;
  /**
   * Optional measure column for `sum`/`avg`/`min`/`max` aggregations. Ignored when `agg` is `count`.
   */
  measure?: string | null;
  vertex_type: string;
}
export interface AggregateResult {
  rows: AggregateRow[];
}
export interface AggregateRow {
  group: unknown;
  value: number;
}
export interface BoundingBox {
  x_max: number;
  x_min: number;
  y_max: number;
  y_min: number;
}
export interface ColumnDescriptor {
  duckdb_type: string;
  name: string;
}
export interface DescribeFieldParams {
  field: string;
  vertex_type: string;
}
export interface DescribeFieldResult {
  datatype: string;
  /**
   * Distinct value count when known from manifest stats.
   */
  distinct?: number | null;
  /**
   * Inferred role for chart-axis defaults: `identifier`, `dimension`, `measure`. Mirrors keasy `lib/graph-schema.ts::inferRole` — promoted here to be authoritative.
   */
  role: "identifier" | "dimension" | "measure";
  /**
   * Up to 8 sample values surfaced by the writer.
   */
  samples: string[];
}
export interface DescribeVertexTypeParams {
  vertex_type: string;
}
export interface DescribeVertexTypeResult {
  /**
   * Total row count of the vertex table (`COUNT(*)`), the denominator role inference uses for the cardinality test.
   */
  count: number;
  /**
   * Every user-facing field (reserved columns filtered), in manifest order, with authoritative role + cardinality. One batched query computes all of it — the single source for what keasy used to derive client-side.
   */
  fields: FieldStat[];
}
export interface FieldStat {
  /**
   * `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
   */
  datatype: string;
  /**
   * Distinct value count (`COUNT(DISTINCT field)`).
   */
  distinct: number;
  name: string;
  /**
   * Authoritative chart-axis role.
   */
  role: "identifier" | "dimension" | "measure";
}
export interface EdgeTypeSummary {
  count: number;
  iri: string;
  name: string;
  source_type: string;
  /**
   * Convenience: `DuckDB` view name `{source}_{name}_{target}`. Pre-computed here so bindings don't reimplement the `GraphAr` edge naming convention.
   */
  table_name: string;
  target_type: string;
}
export interface ExecuteSqlParams {
  /**
   * Hard cap on rows returned to the caller. The executor MUST apply an outer `LIMIT` regardless of what the user's SQL contains.
   */
  row_cap?: number;
  sql: string;
  /**
   * Hard cap on wall-clock execution time, milliseconds.
   */
  timeout_ms?: number;
}
export interface ExecuteSqlResult {
  columns: ColumnDescriptor[];
  rows: unknown[];
  /**
   * True when the result was truncated by `row_cap`.
   */
  truncated: boolean;
}
export interface FindNeighborsParams {
  depth?: number;
  /**
   * Restrict traversal to a subset of edge names. Empty = all.
   */
  edge_types?: string[];
  iri: string;
  limit?: number;
}
export interface FindNeighborsResult {
  edges: NeighborEdge[];
  vertices: NeighborVertex[];
}
export interface NeighborEdge {
  predicate: string;
  source: string;
  target: string;
}
export interface NeighborVertex {
  /**
   * Hop count from the origin (0 = origin itself).
   */
  hop: number;
  iri: string;
  label: string;
  vertex_type: string;
}
export interface FindPathParams {
  max_hops?: number;
  source_iri: string;
  target_iri: string;
}
export interface FindPathResult {
  edges: NeighborEdge[];
  /**
   * Ordered path vertices including endpoints. Empty when no path exists within `max_hops`.
   */
  vertices: NeighborVertex[];
}
export interface GetVertexParams {
  /**
   * The vertex's `subject` IRI (unique across the graph).
   */
  subject: string;
  vertex_type: string;
}
export interface GetVertexResult {
  /**
   * The matched vertex's user-facing property columns (reserved columns filtered) as a JSON object; `null` when no vertex has that subject.
   */
  vertex?: {
    [k: string]: unknown;
  };
}
export interface HistogramParams {
  bins?: number;
  field: string;
  vertex_type: string;
}
export interface HistogramResult {
  /**
   * Per-bin counts.
   */
  counts: number[];
  /**
   * Bin edges (length `bins + 1` for numeric, `bins` for categorical).
   */
  edges: number[];
  /**
   * Field role echoed back so the caller can pick the right chart.
   */
  field_kind: "numeric" | "temporal" | "categorical";
}
export interface ListEdgeTypesParams {}
export interface ListEdgeTypesResult {
  edges: EdgeTypeSummary[];
}
export interface ListVertexTypesParams {}
export interface ListVertexTypesResult {
  types: VertexTypeSummary[];
}
export interface VertexTypeSummary {
  /**
   * Vertex count from the manifest.
   */
  count: number;
  /**
   * Field names for downstream calls to `describe_field`.
   */
  fields: string[];
  /**
   * Full RDF type IRI.
   */
  iri: string;
  /**
   * Short local name as used in `DuckDB` table identifier (e.g. `"Person"`).
   */
  name: string;
}
export interface MaterializeGraphParams {
  limit?: number;
  /**
   * Restrict to a subset of vertex types. Empty = all.
   */
  vertex_types?: string[];
}
export interface MaterializeGraphResult {
  edges: MaterializedEdge[];
  /**
   * True when the vertex set was capped by `limit` (edges to dropped vertices are omitted, mirroring the canvas's orphan-edge drop).
   */
  truncated: boolean;
  vertices: MaterializedVertex[];
}
export interface MaterializedEdge {
  predicate: string;
  /**
   * Source vertex `subject` (dense→subject resolved).
   */
  source: string;
  /**
   * Target vertex `subject` (dense→subject resolved).
   */
  target: string;
}
export interface MaterializedVertex {
  /**
   * The vertex `subject` IRI — the canvas's string node id.
   */
  id: string;
  /**
   * Display label: first present of `name`/`label`/`title`, else the subject.
   */
  label: string;
  /**
   * Vertex type short name.
   */
  type_name: string;
}
export interface TopKParams {
  descending?: boolean;
  k?: number;
  order_by: string;
  vertex_type: string;
}
export interface ViewportParams {
  bbox: BoundingBox;
  limit?: number;
  lod_threshold?: number;
  /**
   * Restrict to a subset of vertex types. Empty = all.
   */
  vertex_types?: string[];
  /**
   * Current zoom level. Above [`Self::lod_threshold`] the executor swaps to aggregate mode (`GROUP BY cluster_id`) returning ≤ 10k super-nodes regardless of total N.
   */
  zoom: number;
}
export interface TopKResult {
  /**
   * Rows as opaque JSON objects so the verb stays generic across arbitrary vertex shapes. Bindings render to their UI of choice.
   */
  rows: unknown[];
}
/**
 * An edge both of whose endpoints are in the answer.
 *
 * Endpoints are indices into `ViewportResult::vertices`, not dense ids — an edge is only emitted when both ends survived the bbox and the limit, because one that reaches off screen has nowhere to land.
 */
export interface ViewportEdge {
  dst_dense: number;
  src_dense: number;
}
export interface ViewportResult {
  edges: ViewportEdge[];
  mode: ViewportMode;
  /**
   * How many vertices **matched**, before `limit` cut them.
   *
   * Separate from `vertices.len()` on purpose: the difference is how a view says "there is more here than I am showing you", and without it a truncated answer looks exactly like a complete one.
   */
  n: number;
  /**
   * Always present; in aggregate mode the entries are super-nodes (cluster centroids) with `cluster_id` populated.
   */
  vertices: ViewportVertex[];
}
export interface ViewportVertex {
  cluster_id?: number | null;
  /**
   * Aggregate mode only: cluster size + `cluster_id`.
   */
  cluster_size?: number | null;
  /**
   * This vertex's position **in this answer** — what `ViewportEdge` refers to and what a GPU buffer is indexed by. See the module note: it is not the `GraphAr` dense id, which is ambiguous across vertex types and does not survive a `LIMIT`.
   */
  dense_id: number;
  type_idx: number;
  x: number;
  y: number;
}
