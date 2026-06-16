/**
 * @fossil-lang/wasm — JS/TS wrapper around the fossil-wasm wasm-bindgen artifacts.
 *
 * Public API (one of the six @fossil-lang/* packages per ADR-0028):
 * - {@link initFossilWasm} — consumer-controlled .wasm URL loader (memoised).
 * - {@link tokenize} — calls the Rust lexer, returns TokenRow[] (ADR-0030).
 * - {@link semanticLegend} — returns the LSP SemanticTokensLegend (Phase-6 06-07).
 * - {@link FossilPlayground} — Workspace API class (ADR-0024) for the LSP.
 *
 * Consumer pattern (wasm-bindgen --target web — RESEARCH.md Pattern 3):
 *
 *   import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
 *   import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';  // Vite
 *
 *   await initFossilWasm({ wasmUrl });
 *   const tokens = tokenize('prefix ex: <https://example.org/>');
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

/** Stdlib classification entry returned by {@link FossilPlayground.classification}.
 *  `wasm_class` indicates whether a stdlib function can execute in-browser
 *  (`"pure_sql"`) or requires the native runtime (`"native_udf_only"`). The
 *  playground reads this once at startup to render `native_udf_only` functions
 *  as disabled (STDL-07 / SC#1 playground half).
 */
export interface StdlibClass {
  name: string;
  wasm_class: 'pure_sql' | 'native_udf_only';
}

/**
 * Canonical primitive names for inferred-descriptor columns. Must match the
 * lookup table in `fossil-hir::infer::primitive_from_name` (ADR-0037). Names
 * map 1:1 to the `Primitive` enum in HIR (Integer, Float, String, Bool, Date,
 * DateTime, Time, GYear, AnyURI).
 *
 * Unknown / non-canonical strings are accepted on the wire — the Rust side
 * coerces them to `String` and emits a `D-INFERRED-UNKNOWN-DATATYPE`
 * diagnostic. Hosts SHOULD canonicalise their DuckDB `DESCRIBE` output to
 * these names before registering.
 */
export type InferredPrimitive =
  | 'String'
  | 'Integer'
  | 'Float'
  | 'Bool'
  | 'Date'
  | 'DateTime'
  | 'Time'
  | 'GYear'
  | 'AnyURI';

/** One column from a host-introspected source. */
export interface InferredColumnJson {
  name: string;
  primitive: InferredPrimitive;
}

/**
 * Host-introspected input schema. Produced by the playground's browser-side
 * `DuckDB-WASM` `DESCRIBE read_csv_auto('<url>')` call (see plan 13-04b);
 * consumed by the Rust compiler via
 * {@link FossilPlayground.registerInferredDescriptor}. See ADR-0037 for the
 * full architectural rationale (drop user-facing CSVW; host-side
 * introspection feeds the compiler ahead of `compile()`).
 */
export interface InferredDescriptorJson {
  /** Source binding name (e.g. `"users"` for `users := io.csv(...)`). */
  source_name: string;
  /** Ordered columns — order is significant for column-position fallback. */
  columns: InferredColumnJson[];
  /**
   * Opaque content-hash. Empty string means "let the Rust side derive a
   * deterministic hash from the column tuple list" (used for Salsa keying).
   * Hosts that already maintain a per-file content-hash (e.g. resolver-side)
   * MAY supply it.
   */
  content_hash: string;
}

// Re-export the Token + Diagnostic + Resolver types for ergonomics — consumers
// can import the full surface from `@fossil-lang/wasm` without also reaching
// for `@fossil-lang/types` (still works; this is convenience).
export type { TokenRow, SemanticTokensLegend } from '@fossil-lang/types';
