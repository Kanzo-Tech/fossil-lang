import { dispatch_graph } from '../pkg/fossil_graph_wasm.js';
import type {
  Operation,
  SchemaParams,
  SchemaResult,
  ReadParams,
  ReadResult,
  ExpandParams,
  ExpandResult,
  PathParams,
  PathResult,
  AggregateParams,
  AggregateResult,
  ExecuteSqlParams,
  ExecuteSqlResult,
} from './generated.js';

/** One row of a DuckDB result, as a plain `{ column: value }` object. */
export type QueryRow = Record<string, unknown>;

/**
 * The host's DuckDB-WASM query callback. Runs the SQL the verb logic emits and
 * resolves the rows as plain objects.
 *
 * In keasy this wraps the Mosaic coordinator, e.g.:
 *
 * ```ts
 * const query: QueryFn = async (sql) => {
 *   const table = await coordinator.query(sql, { type: 'arrow' });
 *   return table.toArray().map((r) => r.toJSON());
 * };
 * ```
 *
 * (Mosaic returns an Arrow table; the binding's WASM core expects row objects,
 * so the host adapts once here — keeping this package free of an Arrow/Mosaic
 * dependency.)
 */
export type QueryFn = (sql: string) => Promise<QueryRow[]>;

/** Options for {@link createGraphClient}. */
export interface CreateGraphClientOpts {
  /** Runs verb SQL on the host's DuckDB-WASM connection. See {@link QueryFn}. */
  query: QueryFn;
  /**
   * The GraphAr manifest YAMLs, keyed by relative path, pre-fetched by the host
   * (httpfs / OPFS / static). They are small (one root + per-type YAMLs); the
   * binding keeps the {@link ManifestSource} sync by taking them by value.
   */
  manifestFiles: Record<string, string>;
}

/**
 * The typed verb surface: six methods, one per `fossil-graph` verb, each
 * forwarding to the WASM `dispatch_graph` with the shared `query` +
 * `manifestFiles`. Params and results are the codegen'd shapes from the
 * schemars JSON Schemas (`./generated.ts`) — single source of truth with the
 * Rust verb structs.
 *
 * There is no viewport method and there will not be one: the camera is
 * addressed, not queried (ADR-0042). These verbs answer questions and return
 * ids; what gets drawn comes from tiles.
 */
export interface GraphClient {
  /**
   * The manifest, and per-field statistics on request. Bare: the vertex and
   * edge types with their counts, and no field is queried. With `vertex_type`:
   * one batched query adds that type's per-field cardinality and role. With
   * `field` too: narrowed to that field, plus its samples — the one shape that
   * pays for a second query.
   */
  schema(params?: SchemaParams): Promise<SchemaResult>;
  /**
   * Rows of one vertex type: a `where` predicate, an order, a limit. The
   * general bounded read — reading one vertex is `where: "subject = '…'"`.
   * `where` is SQL and carries the same authority as `executeSql`: gate it
   * with the same permission.
   */
  read(params: ReadParams): Promise<ReadResult>;
  /**
   * The neighbourhood of a set of vertices: `all` walks outward up to `depth`,
   * `into` keeps only the edges whose both ends are in the set.
   */
  expand(params: ExpandParams): Promise<ExpandResult>;
  /** The shortest route between two vertices. */
  path(params: PathParams): Promise<PathResult>;
  /**
   * One grouping, over values or — with `bins` — over equal-width ranges of
   * the column. Binning is grouping, so there is no separate histogram verb.
   */
  aggregate(params: AggregateParams): Promise<AggregateResult>;
  executeSql(params: ExecuteSqlParams): Promise<ExecuteSqlResult>;
  /** Escape hatch: dispatch a raw `{ verb, params }` operation. */
  dispatch(op: Operation): Promise<unknown>;
}

/**
 * Create a {@link GraphClient} over a DuckDB-WASM `query` callback and the
 * pre-fetched GraphAr manifest.
 *
 * {@link initFossilGraphWasm} MUST have resolved before any verb call — the
 * WASM `dispatch_graph` throws otherwise.
 */
export function createGraphClient(opts: CreateGraphClientOpts): GraphClient {
  const { query, manifestFiles } = opts;

  const dispatch = (op: Operation): Promise<unknown> =>
    // dispatch_graph(op, manifest_files, query) → Promise<Result>. The Rust side
    // calls `query.call1(null, sql)`, so an arrow fn (no `this`) is correct.
    dispatch_graph(op, manifestFiles, query as unknown as (sql: string) => Promise<QueryRow[]>);

  const call = async <R>(op: Operation): Promise<R> => (await dispatch(op)) as R;

  return {
    schema: (params = {}) => call({ verb: 'schema', params }),
    read: (params) => call({ verb: 'read', params }),
    expand: (params) => call({ verb: 'expand', params }),
    path: (params) => call({ verb: 'path', params }),
    aggregate: (params) => call({ verb: 'aggregate', params }),
    executeSql: (params) => call({ verb: 'execute_sql', params }),
    dispatch,
  };
}
