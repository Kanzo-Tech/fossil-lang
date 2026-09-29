/* eslint-disable */
/**
 * GENERATED — do not edit by hand.
 * Source: crates/fossil-graph JSON Schemas (schemars). Regenerate with
 *   pnpm --filter @fossil-lang/corpus gen:types
 */

/**
 * What kind of VALUE a field holds, read off its `GraphAr` data type alone.
 *
 * Orthogonal to [`FieldRole`], which also weighs the name and the cardinality: an `int64` `user_id` is an `Identifier` by role and `Numeric` by kind. The kind is what decides whether `aggregate` may bin the field (`Numeric` and `Temporal` have ranges, `Categorical` groups by value only) and whether an axis is a time axis. It is the one table of `GraphAr` spellings; a reader asks for the kind instead of keeping a copy of it.
 */
export type FieldKind = "numeric" | "temporal" | "categorical";
/**
 * Inferred role for chart-axis defaults. Mirrors keasy `lib/graph-schema.ts:: inferRole` — promoted here to be authoritative.
 */
export type FieldRole = "identifier" | "dimension" | "measure";
/**
 * All graph operations dispatchable on the surface.
 *
 * The `tag = "verb"` serde representation makes the wire form `{ "verb": "schema", "params": { … } }`, whatever transport carries it.
 *
 * # It serialises, and it does not deserialise
 *
 * There is no `Deserialize` impl, and its absence is load-bearing rather than an oversight: `execute_sql` carries [`RawSql`], and [`Operation::from_wire`] is the one door from wire JSON because that door takes the permission. [`raw_sql`] is the argument.
 */
export type Operation =
  | {
      params: SchemaParams;
      verb: "schema";
    }
  | {
      params: ExecuteSqlParams;
      verb: "execute_sql";
    };

export interface FossilGraphSchemas {
  ColumnDescriptor?: ColumnDescriptor;
  EdgeTypeSummary?: EdgeTypeSummary;
  ExecuteSqlParams?: ExecuteSqlParams;
  ExecuteSqlResult?: ExecuteSqlResult;
  FieldKind?: FieldKind;
  FieldRole?: FieldRole;
  FieldStat?: FieldStat;
  Operation?: Operation;
  SchemaParams?: SchemaParams;
  SchemaResult?: SchemaResult;
  VertexTypeSummary?: VertexTypeSummary;
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
export interface FieldStat {
  /**
   * `GraphAr` data-type spelling (`string`, `int64`, `double`, …).
   */
  datatype: string;
  /**
   * Distinct value count (`COUNT(DISTINCT field)`).
   */
  distinct: number;
  /**
   * What an axis over the field can do with it — see [`FieldKind`].
   */
  kind: "numeric" | "temporal" | "categorical";
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
   * Every vertex type's per-field statistics, on its [`VertexTypeSummary::stats`] — one batched query per type, the same one `vertex_type` spends on one. What a host drawing a schema panel asks for instead of one call per type. Samples are never part of it.
   */
  stats?: boolean;
  /**
   * Name a vertex type to also get its per-field statistics. Omitted, the answer is the type lists alone and no field is queried.
   */
  vertex_type?: string | null;
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
   * **Not from the manifest, despite what this said.** The executor answers it with `count_rows` — a `SELECT count(*)` over the type's tiles. The manifest carries a declared `vertex_count` now, so the query is a second way to ask one question and can go; what it buys until then is that it measures the bytes rather than trusting the declaration, which is the disagreement `packages/corpus/guards`' `declared-count` guard exists to catch.
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
  /**
   * Per-field statistics for this type, in the order of [`Self::fields`]. **Empty unless the call set `stats`**; with it, empty only for a type that declares no field.
   */
  stats: FieldStat[];
}
