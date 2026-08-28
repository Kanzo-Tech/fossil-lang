/* eslint-disable */
/**
 * GENERATED — do not edit by hand.
 * Source: crates/fossil-graph JSON Schemas (schemars). Regenerate with
 *   pnpm --filter @fossil-lang/corpus gen:types
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
 *
 * # It serialises, and it does not deserialise
 *
 * There is no `Deserialize` impl, and its absence is load-bearing rather than an oversight: two variants carry [`RawSql`], and [`Operation::from_wire`] is the one door from wire JSON because that door takes the permission those two fields share. [`raw_sql`] is the argument.
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
      params: ExecuteSqlParams;
      verb: "execute_sql";
    };

export interface FossilGraphSchemas {
  AggregateParams?: AggregateParams;
  AggregateResult?: AggregateResult;
  AggregateRow?: AggregateRow;
  Aggregation?: Aggregation;
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
  Operation?: Operation;
  PathParams?: PathParams;
  PathResult?: PathResult;
  ReadParams?: ReadParams;
  ReadResult?: ReadResult;
  SchemaParams?: SchemaParams;
  SchemaResult?: SchemaResult;
  VertexTypeSummary?: VertexTypeSummary;
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
 * **`where` is SQL and is trusted exactly as far as `execute_sql` is**, which is why its type is [`RawSql`] and why this struct is not `Deserialize`: the permission that opens the escape hatch is the same token that fills this field, and [`Operation::from_wire`](super::Operation::from_wire) is the one place either can be built from wire JSON. [`raw_sql`](super::raw_sql) carries the argument.
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
   * How many vertices the type has.
   *
   * **Not from the manifest, despite what this said.** The executor answers it with `count_rows` — a `SELECT count(*)` over the type's tiles. The manifest carries a declared `vertex_count` now, so the query is a second way to ask one question and can go; what it buys until then is that it measures the bytes rather than trusting the declaration, which is the disagreement `apps/corpus`'s `declared-count` guard exists to catch.
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
