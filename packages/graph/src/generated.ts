/* eslint-disable */
/**
 * GENERATED — do not edit by hand.
 * Source: crates/fossil-graph JSON Schemas (schemars). Regenerate with
 *   pnpm --filter @fossil-lang/graph gen:types
 */

export type Aggregation = "count" | "sum" | "avg" | "min" | "max";
/**
 * Which edges an expansion keeps — Neo4j's `Expand(All)` / `Expand(Into)`.
 */
export type ExpandMode = "all" | "into";
/**
 * Inferred role for chart-axis defaults. Mirrors keasy `lib/graph-schema.ts:: inferRole` — promoted here to be authoritative.
 */
export type FieldRole = "identifier" | "dimension" | "measure";
/**
 * All graph operations dispatchable on the surface.
 *
 * The `tag = "verb"` serde representation makes the wire form `{ "verb": "schema", "params": { … } }` — identical for MCP tool calls, HTTP POST bodies, and CLI subcommand args.
 */
export type Operation =
  | {
      params: SchemaParams;
      verb: "schema";
    }
  | {
      params: ReadParams;
      verb: "read";
    }
  | {
      params: ExpandParams;
      verb: "expand";
    }
  | {
      params: PathParams;
      verb: "path";
    }
  | {
      params: AggregateParams;
      verb: "aggregate";
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
  EdgeTypeSummary?: EdgeTypeSummary;
  ExecuteSqlParams?: ExecuteSqlParams;
  ExecuteSqlResult?: ExecuteSqlResult;
  ExpandMode?: ExpandMode;
  ExpandParams?: ExpandParams;
  ExpandResult?: ExpandResult;
  FieldRole?: FieldRole;
  FieldStat?: FieldStat;
  GraphEdge?: GraphEdge;
  GraphVertex?: GraphVertex;
  MaterializeGraphParams?: MaterializeGraphParams;
  MaterializeGraphResult?: MaterializeGraphResult;
  MaterializedEdge?: MaterializedEdge;
  MaterializedVertex?: MaterializedVertex;
  Operation?: Operation;
  PathParams?: PathParams;
  PathResult?: PathResult;
  ReadParams?: ReadParams;
  ReadResult?: ReadResult;
  SchemaParams?: SchemaParams;
  SchemaResult?: SchemaResult;
  VertexTypeSummary?: VertexTypeSummary;
  ViewportEdge?: ViewportEdge;
  ViewportMode?: ViewportMode;
  ViewportParams?: ViewportParams;
  ViewportResult?: ViewportResult;
  ViewportVertex?: ViewportVertex;
}
/**
 * One grouping, over values or over ranges.
 *
 * **Binning is grouping**, which is why `histogram` is not a second verb: it was the same `GROUP BY` with the key computed from a range instead of read from a column. Setting [`Self::bins`] is what picks which, and it is the only difference between the two.
 */
export interface AggregateParams {
  agg: Aggregation;
  /**
   * Group over this many equal-width ranges of `group_by` rather than over its distinct values — what `histogram` used to be. Requires a numeric or temporal column: a categorical one has no ranges, and grouping it by value is already the answer.
   */
  bins?: number | null;
  /**
   * The column the groups come from — its values, or its ranges when [`Self::bins`] is set.
   */
  group_by: string;
  /**
   * Cap on rows returned. Groups are ordered by value and cut here; a binned call is cut to this many bins instead, so `limit` means one thing.
   */
  limit?: number;
  /**
   * Optional measure column for `sum`/`avg`/`min`/`max` aggregations. Ignored when `agg` is `count`.
   */
  measure?: string | null;
  vertex_type: string;
}
export interface AggregateResult {
  /**
   * Bin boundaries, `rows.len() + 1` of them, low to high. **Empty unless the call set `bins`** — a grouping over values has no axis to draw.
   */
  edges: number[];
  rows: AggregateRow[];
}
export interface AggregateRow {
  /**
   * The group's key: the column's value, or the bin's ordinal when the call was binned (pair it with `AggregateResult::edges` for the range).
   */
  group: {
    [k: string]: unknown;
  };
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
export interface ExpandParams {
  /**
   * Hops to walk. [`ExpandMode::Into`] ignores it — the induced subgraph has no frontier to advance.
   */
  depth?: number;
  /**
   * Restrict traversal to a subset of edge names. Empty = all.
   */
  edge_types?: string[];
  /**
   * The vertices to expand from, by subject IRI.
   */
  from: string[];
  limit?: number;
  /**
   * Which edges an expansion keeps — Neo4j's `Expand(All)` / `Expand(Into)`.
   */
  mode?: "all" | "into";
}
export interface ExpandResult {
  edges: GraphEdge[];
  vertices: GraphVertex[];
}
export interface GraphEdge {
  predicate: string;
  source: string;
  target: string;
}
export interface GraphVertex {
  /**
   * Hop count from the origin set (0 = a vertex the call named).
   */
  hop: number;
  iri: string;
  label: string;
  vertex_type: string;
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
  /**
   * Up to 8 non-null values. **Populated only when the call named this field**: they are a second query, and a bare per-type call would pay it once per column.
   */
  samples: string[];
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
export interface SchemaParams {
  /**
   * Name a field to narrow the statistics to it and pick up its samples. Ignored without `vertex_type`.
   */
  field?: string | null;
  /**
   * Name a vertex type to also get its per-field statistics. Omitted, the answer is the type lists alone and no field is queried.
   */
  vertex_type?: string | null;
}
/**
 * Rows of one vertex type, filtered, ordered and capped.
 *
 * **`where` is SQL and is trusted exactly as far as `execute_sql` is.** A binding that gates the escape hatch behind a permission MUST gate this field with it: the two carry the same authority over the same engine.
 */
export interface ReadParams {
  descending?: boolean;
  limit?: number;
  /**
   * Column to order by. Absent, the rows arrive in storage order, which is the writer's Morton order and says nothing the caller asked about.
   */
  order_by?: string | null;
  vertex_type: string;
  /**
   * A `WHERE` predicate over the type's columns, without the keyword. Reading one vertex is `subject = '…'`.
   */
  where?: string | null;
}
export interface PathParams {
  max_hops?: number;
  source_iri: string;
  target_iri: string;
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
export interface PathResult {
  edges: GraphEdge[];
  /**
   * Ordered path vertices including endpoints. Empty when no path exists within `max_hops`.
   */
  vertices: GraphVertex[];
}
export interface ReadResult {
  /**
   * Rows as opaque JSON objects so the verb stays generic across arbitrary vertex shapes. `subject` rides along as the identity; the writer's layout columns do not — those are the tiles' business, not the algebra's.
   */
  rows: unknown[];
}
export interface SchemaResult {
  edges: EdgeTypeSummary[];
  /**
   * Per-field statistics for the named `vertex_type`, narrowed to `field` when one was named. **Empty when no `vertex_type` was named** — that is the whole of the cheap/expensive distinction.
   */
  fields: FieldStat[];
  vertices: VertexTypeSummary[];
}
export interface VertexTypeSummary {
  /**
   * Vertex count from the manifest.
   */
  count: number;
  /**
   * Field names — what a follow-up `schema { vertex_type, field }` may name.
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
