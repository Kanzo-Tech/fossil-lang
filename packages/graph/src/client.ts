import { dispatch_graph } from '../pkg/fossil_graph_wasm.js';
import type {
  Operation,
  ListVertexTypesParams,
  ListVertexTypesResult,
  ListEdgeTypesParams,
  ListEdgeTypesResult,
  DescribeFieldParams,
  DescribeFieldResult,
  DescribeVertexTypeParams,
  DescribeVertexTypeResult,
  SearchByLabelParams,
  SearchByLabelResult,
  FindNeighborsParams,
  FindNeighborsResult,
  FindPathParams,
  FindPathResult,
  GetVertexParams,
  GetVertexResult,
  AggregateParams,
  AggregateResult,
  HistogramParams,
  HistogramResult,
  TopKParams,
  TopKResult,
  SummarizeClusterParams,
  SummarizeClusterResult,
  AnswerWithCommunitiesParams,
  AnswerWithCommunitiesResult,
  ViewportParams,
  ViewportResult,
  SetSelectionParams,
  SetSelectionResult,
  MaterializeGraphParams,
  MaterializeGraphResult,
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
 * The typed verb surface. One method per `fossil-graph` verb; each forwards to
 * the WASM `dispatch_graph` with the shared `query` + `manifestFiles`. Params
 * and results are the codegen'd shapes from the schemars JSON Schemas
 * (`./generated.ts`) — single source of truth with the Rust verb structs.
 */
export interface GraphClient {
  listVertexTypes(params?: ListVertexTypesParams): Promise<ListVertexTypesResult>;
  listEdgeTypes(params?: ListEdgeTypesParams): Promise<ListEdgeTypesResult>;
  describeField(params: DescribeFieldParams): Promise<DescribeFieldResult>;
  /** Batched per-type field stats + authoritative roles in one call. */
  describeVertexType(params: DescribeVertexTypeParams): Promise<DescribeVertexTypeResult>;
  searchByLabel(params: SearchByLabelParams): Promise<SearchByLabelResult>;
  findNeighbors(params: FindNeighborsParams): Promise<FindNeighborsResult>;
  findPath(params: FindPathParams): Promise<FindPathResult>;
  /** Fetch one vertex's user-facing properties by subject IRI. */
  getVertex(params: GetVertexParams): Promise<GetVertexResult>;
  aggregate(params: AggregateParams): Promise<AggregateResult>;
  histogram(params: HistogramParams): Promise<HistogramResult>;
  topK(params: TopKParams): Promise<TopKResult>;
  summarizeCluster(params: SummarizeClusterParams): Promise<SummarizeClusterResult>;
  answerWithCommunities(
    params: AnswerWithCommunitiesParams,
  ): Promise<AnswerWithCommunitiesResult>;
  viewport(params: ViewportParams): Promise<ViewportResult>;
  setSelection(params: SetSelectionParams): Promise<SetSelectionResult>;
  /** Canvas-ready whole-graph snapshot (vertices + dense→subject-mapped edges). */
  materializeGraph(params: MaterializeGraphParams): Promise<MaterializeGraphResult>;
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
    listVertexTypes: (params = {}) => call({ verb: 'list_vertex_types', params }),
    listEdgeTypes: (params = {}) => call({ verb: 'list_edge_types', params }),
    describeField: (params) => call({ verb: 'describe_field', params }),
    describeVertexType: (params) => call({ verb: 'describe_vertex_type', params }),
    searchByLabel: (params) => call({ verb: 'search_by_label', params }),
    findNeighbors: (params) => call({ verb: 'find_neighbors', params }),
    findPath: (params) => call({ verb: 'find_path', params }),
    getVertex: (params) => call({ verb: 'get_vertex', params }),
    aggregate: (params) => call({ verb: 'aggregate', params }),
    histogram: (params) => call({ verb: 'histogram', params }),
    topK: (params) => call({ verb: 'top_k', params }),
    summarizeCluster: (params) => call({ verb: 'summarize_cluster', params }),
    answerWithCommunities: (params) => call({ verb: 'answer_with_communities', params }),
    viewport: (params) => call({ verb: 'viewport', params }),
    setSelection: (params) => call({ verb: 'set_selection', params }),
    materializeGraph: (params) => call({ verb: 'materialize_graph', params }),
    executeSql: (params) => call({ verb: 'execute_sql', params }),
    dispatch,
  };
}
