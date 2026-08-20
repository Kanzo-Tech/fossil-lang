/**
 * @fossil-lang/wasm — JS/TS wrapper around the fossil-wasm wasm-bindgen artifacts.
 *
 * Public API (one of the @fossil-lang/* packages; `git ls-files packages` is
 * the list):
 * - {@link initFossilWasm} — consumer-controlled .wasm URL loader (memoised).
 * - {@link tokenize} — calls the Rust lexer, returns TokenRow[].
 * - {@link semanticLegend} — returns the LSP SemanticTokensLegend.
 * - {@link FossilPlayground} — Workspace API class for the LSP.
 *
 * Consumer pattern (wasm-bindgen --target web):
 *
 *   import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
 *   import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';  // Vite
 *
 *   await initFossilWasm({ wasmUrl });
 *   const tokens = tokenize('User := io.csv("data/people.csv")');
 */

// The wasm-bindgen glue (`../pkg/fossil_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `client.ts` (the lexer / LSP / Workspace API) —
// never here in the entry. See `client.ts`: importing the stateful glue from the
// package entry lets a `sideEffects:false` bundler duplicate it, splitting
// `init()`'s wasm instance from the one the functions use (the crash the
// codemirror tokenizer hit on the main thread). This entry only re-exports.
export { initFossilWasm } from './load.js';
export type { InitFossilWasmOpts } from './load.js';

export {
  start_lsp_worker,
  tokenize,
  semanticLegend,
  FossilPlayground,
  refs,
  providers,
} from './client.js';
export type { FileHandle } from './client.js';

/** One external reference a program makes — the typed lineage returned by
 *  {@link refs}. Mirrors `fossil_run_status::SourceRefInfo` (the SAME struct the
 *  native `fossil refs` emits). `connection` is the `@conn` alias the reference
 *  targets, or `null` for a direct URL/path; `role` is where it appears in the
 *  source constructor. The host resolves `@conn` → `{base}/path` itself. */
export interface SourceRefInfo {
  connection: string | null;
  path: string;
  role: 'data' | 'schema';
}

/** One data-source provider returned by {@link providers}. Mirrors
 *  `fossil_run_status::ProviderInfo`: the short name (`csv`, `rdf`, …), the file
 *  extensions it reads (no leading dot), and how it can be used. */
export interface ProviderInfo {
  name: string;
  extensions: string[];
  kind: 'schema' | 'data' | 'both';
}

/** Diagnostic row returned by {@link FossilPlayground.check} and
 *  {@link FossilPlayground.diagnosticsFor} — mirrors `fossil-wasm`'s
 *  `CheckRow` JSON shape (the LSP `Diagnostic` projected through
 *  `fossil_ide::LineIndex` for UTF-16 ranges).
 */
export interface CheckRow {
  uri: string;
  range: {
    start: { line: number; character: number };
    end: { line: number; character: number };
  };
  severity: number;
  message: string;
}

// `StdlibClass` and `FossilPlayground.classification()` lived here — one row per
// stdlib function carrying `"pure_sql"` or `"native_udf_only"`, so a playground
// could render the native-only functions as disabled. The Rust side deleted the
// concept (`crates/fossil-wasm/src/lib.rs`, two tombstones, and
// `crates/fossil-hir/src/stdlib.rs`): every catalogued function is a pure SQL
// expression template, so there is nothing to disable. This binding kept calling
// it, and only type-checked because `pkg/fossil_wasm.d.ts` is a gitignored build
// output that had not been regenerated since — it declared `classification(): any`
// while the method it described no longer existed.

/**
 * The primitive lattice, as `fossil-graph-schema` serialises it — the same enum
 * the checker types against, not a set of names it looks up.
 *
 * A value outside this union is REJECTED when the descriptor is registered:
 * `registerInferredDescriptor` returns the serde error naming the offending
 * value. It is no longer coerced to `string` with a diagnostic three crates
 * later, so a host that sends a type fossil does not carry finds out at the
 * call, not in a compile.
 */
export type InferredPrimitive =
  | 'string'
  | 'integer'
  | 'float'
  | 'bool'
  | 'date'
  | 'date_time'
  | 'time'
  | 'g_year'
  | 'any_uri';

/** One column from a host-introspected source. */
export interface InferredColumnJson {
  name: string;
  primitive: InferredPrimitive;
}

/**
 * Host-introspected input schema. Produced by a `DESCRIBE SELECT * FROM
 * <reader>('<url>')` the host runs — `DuckDB-WASM` in a browser — where the
 * reader is the one the binding's `io.` constructor names: `read_csv_auto`,
 * `read_json_auto` or `read_parquet`. `@fossil-lang/introspect` is the one home
 * for that, and picking the reader off the constructor is not a nicety: a
 * Parquet file read as CSV fails DuckDB's sniffer outright, and a JSON array
 * read as CSV introspects to a single column named `[`.
 *
 * Consumed by the Rust compiler via
 * {@link FossilPlayground.registerInferredDescriptor}. There is no metadata
 * sidecar for the user to write and keep in sync: the host introspects the
 * real file and feeds the compiler ahead of `compile()`.
 */
export interface InferredDescriptorJson {
  /**
   * The source URI exactly as the program writes it — the string inside
   * `io.csv("examples/users.csv")`. NOT the binding name, and NOT the URL the
   * host resolved in order to read the file: the checker only ever sees what
   * the program says.
   */
  uri: string;
  /** Ordered columns — order is significant for column-position fallback. */
  columns: InferredColumnJson[];
  /**
   * Opaque token identifying the state of the source this was read from. The
   * cache compares it and nothing interprets it: an ETag, a digest, a
   * `Last-Modified`, whatever the host can get cheaply. Empty means "I cannot
   * tell", which the cache reads as never-fresh, so that source is
   * re-introspected on every compile.
   */
  freshness_token: string;
}

// Re-export the Token + Diagnostic + Resolver types for ergonomics — consumers
// can import the full surface from `@fossil-lang/wasm` without also reaching
// for `@fossil-lang/types` (still works; this is convenience).
export type { TokenRow, SemanticTokensLegend } from '@fossil-lang/types';
