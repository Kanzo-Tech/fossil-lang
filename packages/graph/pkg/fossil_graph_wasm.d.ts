/* tslint:disable */
/* eslint-disable */

/**
 * A corpus resolved to addresses — synchronous, and it opens no byte.
 *
 * The counterpart of `resolveCorpus` in `@fossil-lang/graph/address`, and the
 * same arithmetic the native reader runs. A host holds one of these for as long
 * as it holds the manifest.
 */
export class Corpus {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * The file tile `k` of one orientation of one edge type is in, or `null`
     * when the corpus does not publish that orientation.
     *
     * `null` rather than a string is the point: an orientation the manifest does
     * not declare has no address, and handing back a URL that 404s is the failure
     * this whole seam exists to prevent.
     *
     * # Errors
     *
     * A `JsError` when the edge type is not in the manifest, or `direction` is
     * neither `src` nor `dst`.
     */
    adjacencyTileUrl(edge_type: string, direction: string, tile: number): string | undefined;
    /**
     * Resolve `{ rel_path: yaml }` — the manifest set the host pre-fetched —
     * against `base`, which is prepended to every URL and nothing else happens
     * to it.
     *
     * # Errors
     *
     * A `JsError` when the manifest cannot address itself: a missing file, a
     * `chunk_size` no shift addresses, an endpoint type the index does not
     * declare, or an edge whose declared tile size disagrees with the vertex
     * type that addresses it. **Not** for an orientation the corpus does not
     * publish — that is a legitimate corpus, reported by
     * [`Corpus::adjacency_tile_url`] as an address that does not exist rather
     * than one that 404s.
     */
    constructor(manifest_files: any, base?: string | null);
    /**
     * The whole resolution as plain JS data: the container, every vertex type
     * with its prefix, tile size, shift, count and index, and every edge type
     * with the orientations it publishes.
     *
     * # Errors
     *
     * A `JsError` if the resolution cannot be serialised.
     */
    snapshot(): any;
    /**
     * The tile a `dense_id` lives in — the whole of the addressing scheme.
     *
     * **A decimal string in and out.** A `dense_id` may carry more than 53 bits,
     * and JavaScript's `>>` truncates to 32 *before* it shifts, so the same three
     * characters mean something different on each side of this boundary. The
     * published border vectors (`apps/corpus/guards/vectors.json`) are 2^31,
     * where a port that took the shift as signed gives a negative tile, and 2^53,
     * where one that went through a `Number` stops being exact.
     *
     * # Errors
     *
     * A `JsError` when the type is not in the manifest, or `dense_id` is not a
     * decimal integer.
     */
    tileOf(vertex_type: string | null | undefined, dense_id: string): string;
    /**
     * The file tile `k` of a vertex type is in.
     *
     * # Errors
     *
     * A `JsError` when the type is not in the manifest.
     */
    vertexTileUrl(vertex_type: string | null | undefined, tile: number): string;
    /**
     * The URLs a set of vertex tiles addresses, and what that set is complete
     * for. `directions` of `["src"]` is the drawing read; both orientations is
     * the incident set.
     *
     * # Errors
     *
     * A `JsError` when the type is not in the manifest, or a direction is neither
     * `src` nor `dst`.
     */
    window(vertex_type: string | null | undefined, tiles: Uint32Array, directions: string[]): any;
}

/**
 * Dispatch one graph verb in the browser.
 *
 * - `op` — the [`Operation`] as `{ verb, params }` JSON.
 * - `manifest_files` — `{ rel_path: yaml_string }` for the `GraphAr` manifest
 *   (the host pre-fetches these; they're small).
 * - `query` — `(sql: string) => Promise<rows>` running on the host's DuckDB-WASM.
 *
 * Returns the verb's `Result` as a JS value. The `#[wasm_bindgen] async fn`
 * surfaces to JS as a `Promise`.
 *
 * # Errors
 *
 * Returns a `JsError` if `op`/`manifest_files` fail to deserialise, the manifest
 * is invalid, or the verb's execution (including the JS `query` callback) fails.
 */
export function dispatch_graph(op: any, manifest_files: any, query: Function): Promise<any>;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly dispatch_graph: (a: any, b: any, c: any) => any;
    readonly __wbg_corpus_free: (a: number, b: number) => void;
    readonly corpus_new: (a: any, b: number, c: number) => [number, number, number];
    readonly corpus_snapshot: (a: number) => [number, number, number];
    readonly corpus_tileOf: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly corpus_vertexTileUrl: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly corpus_adjacencyTileUrl: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly corpus_window: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly wasm_bindgen__convert__closures_____invoke__hb14cea05d2f56c3d: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h83b90c36f6756d88: (a: number, b: number, c: any, d: any) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
