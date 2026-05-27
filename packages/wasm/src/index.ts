/**
 * @fossil-lang/wasm — JS/TS wrapper around the fossil-wasm wasm-bindgen artifacts.
 *
 * Public API (one of the six @fossil-lang/* packages per ADR-0028):
 * - {@link initFossilWasm} — consumer-controlled .wasm URL loader (memoised).
 * - {@link tokenize} — calls the Rust lexer, returns TokenRow[] (ADR-0030).
 * - {@link semanticLegend} — returns the LSP SemanticTokensLegend (Phase-6 06-07).
 * - {@link FossilPlayground} — Workspace API class (ADR-0024) for LSP + compile.
 *
 * Consumer pattern (wasm-bindgen --target web — RESEARCH.md Pattern 3):
 *
 *   import { initFossilWasm, tokenize } from '@fossil-lang/wasm';
 *   import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';  // Vite
 *
 *   await initFossilWasm({ wasmUrl });
 *   const tokens = tokenize('prefix ex: <https://example.org/>');
 */

// '../pkg/fossil_wasm.js' is a wasm-bindgen --target web output emitted by `pnpm run build:wasm`.
// Gitignored at the repo root (`packages/wasm/pkg/`) but always present at build time. The accompanying
// .d.ts is consumed via the file's `/* @ts-self-types="./fossil_wasm.d.ts" */` pragma.
import {
  FossilPlayground as RawFossilPlayground,
  FileHandle as RawFileHandle,
  tokenize as rawTokenize,
  semantic_legend as rawSemanticLegend,
  start_lsp_worker as rawStartLspWorker,
} from '../pkg/fossil_wasm.js';
import type { TokenRow, SemanticTokensLegend } from '@fossil-lang/types';

export { initFossilWasm } from './load.js';
export type { InitFossilWasmOpts } from './load.js';

/**
 * Install the LSP-over-postMessage dispatcher on the current Worker scope.
 *
 * Per ADR-0024 (`fossil-wasm` IS the LSP server-side) + Phase 7 plan 07-03
 * (the 16-route dispatch loop). The Rust function (re-exported from
 * `crates/fossil-wasm/src/lsp_worker.rs`) installs `self.onmessage` on the
 * Worker scope and owns LSP JSON-RPC dispatch from that point forward —
 * including per-file `textDocument/publishDiagnostics` drains (B3 fix from
 * 07-03), `textDocument/semanticTokens/full` (06-07), hover, completion,
 * definition, references, rename, formatting, code actions, document
 * symbols, signature help.
 *
 * MUST be called inside a Web Worker scope, AFTER {@link initFossilWasm} has
 * resolved. Calling it on the main thread is a no-op (the dispatcher needs
 * `DedicatedWorkerGlobalScope.onmessage`).
 *
 * Consumed by `@fossil-lang/playground`'s `src/workers/lsp.worker.ts` —
 * the React component's LSP Worker entry boots the WASM module then calls
 * this function once.
 */
export function start_lsp_worker(): void {
  rawStartLspWorker();
}

/**
 * Opaque file-handle returned by {@link FossilPlayground.openFile}. Pass it
 * back into the matching `updateFile` / `closeFile` / `diagnosticsFor` /
 * `compileFile` calls. JS code cannot construct one directly (the wasm-bindgen
 * class has a private constructor) — that is intentional per ADR-0024:
 * handles are minted only by `openFile` on the Rust side, where they index a
 * `HashMap<FileHandle, SourceFile>` keyed by a `u32` newtype.
 */
export type FileHandle = RawFileHandle;

/**
 * Tokenize a Fossil source string. Returns the byte-range tokens from the
 * canonical Rust lexer (`fossil_syntax::lexer::raw_lex`) — single grammar
 * source of truth per ADR-0030.
 *
 * MUST be called after {@link initFossilWasm} has resolved; otherwise the
 * underlying wasm-bindgen function throws (the wasm module is not yet
 * instantiated).
 */
export function tokenize(text: string): TokenRow[] {
  // The wasm-bindgen wrapper returns a `JsValue` typed as `any`; the Rust side
  // (crates/fossil-wasm/src/tokenize.rs) serializes `Vec<TokenRow>` via
  // `serde_wasm_bindgen::to_value`, so the shape matches `{ kind, start, end }`
  // exactly. Cast is safe because the Rust ↔ JS contract is enforced upstream.
  return rawTokenize(text) as TokenRow[];
}

/**
 * Returns the LSP `SemanticTokensLegend` (token types + modifiers list) the
 * editor uses to map LSP `semanticTokens/full` response indices to highlight
 * categories. Re-export of `fossil_ide::semantic_legend` (Phase-6 06-07) over
 * the wasm-bindgen boundary — NO duplicate legend definition lives here.
 *
 * MUST be called after {@link initFossilWasm} has resolved.
 */
export function semanticLegend(): SemanticTokensLegend {
  return rawSemanticLegend() as SemanticTokensLegend;
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

/** Compile result returned by {@link FossilPlayground.compile} and
 *  {@link FossilPlayground.compileFile} — the same `{ sql, manifest_yaml }`
 *  shape `fossil-cli`'s compile subcommand produces, minus the native DuckDB
 *  execution step (which runs in-browser via DuckDB-WASM downstream).
 */
export interface CompileResult {
  sql: string;
  manifest_yaml: string;
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

/**
 * Workspace API class (ADR-0024). Thin TS wrapper around the wasm-bindgen
 * `FossilPlayground` that exposes camelCase method names for JS idiom + better
 * TS inference (the raw bindings use snake_case from the Rust impl block).
 *
 * One instance per browser tab / Node process — the instance owns the Salsa
 * store + `Arc<OutputDescriptorKind>` for the schema slot. Subsequent
 * `openFile` calls intern fresh `SourceFile` inputs under the same Salsa
 * revision so `updateFile` benefits from incremental memoisation.
 *
 * The Phase 7 LSP Worker (07-03) drives this class via postMessage; the React
 * component in `@fossil-lang/playground` (08-09) does the same via the
 * WorkerTransport adapter.
 *
 * NOTE: {@link FileHandle} is opaque — JS cannot construct one. Callers receive
 * a handle from `openFile` and pass it back to subsequent operations. The Rust
 * side keeps a `HashMap<FileHandle, SourceFile>` indexed by a `u32` newtype;
 * the wasm-bindgen wrapper exposes that as a class with `private constructor()`.
 */
export class FossilPlayground {
  private _inner: RawFossilPlayground;

  constructor() {
    this._inner = new RawFossilPlayground();
  }

  /**
   * Free the underlying WASM-side Salsa store. Call when the playground is no
   * longer needed (e.g. component unmount). Idempotent in spirit — subsequent
   * method calls on a freed playground throw.
   */
  free(): void {
    this._inner.free();
  }

  /**
   * Compile a Fossil source string ad-hoc (no file lifecycle). Returns
   * `{ sql, manifest_yaml }` — the same shape `fossil-cli compile` produces.
   * Prefer {@link compileFile} for the playground run path (it benefits from
   * the file's stable Salsa identity across edits).
   */
  compile(source: string): CompileResult {
    return this._inner.compile(source) as CompileResult;
  }

  /**
   * Return the stdlib classification manifest (STDL-07). The playground reads
   * this once at startup to render `native_udf_only` functions as disabled
   * with a "native-only — unavailable in the browser" tooltip.
   */
  classification(): StdlibClass[] {
    return this._inner.classification() as StdlibClass[];
  }

  /**
   * Open a file in the workspace. Returns the {@link FileHandle} subsequent
   * `updateFile` / `closeFile` / `diagnosticsFor` / `compileFile` calls key
   * on. `path` is the URI / virtual path diagnostics carry back to the LSP
   * client (e.g. `"file:///tmp/a.fossil"` or `"untitled:Untitled-1"`).
   *
   * Mirrors `ty_wasm::Workspace::open_file` (Astral). Interns a fresh
   * `fossil_base::SourceFile` under the current Salsa revision.
   */
  openFile(path: string, contents: string): FileHandle {
    return this._inner.open_file(path, contents);
  }

  /**
   * Apply an edit to an open file. Mutates the SAME `SourceFile` via the
   * Salsa `Setter` (`set_text`) — this BUMPS THE REVISION (ADR-0022), the
   * real cancellation trigger. NO new `SourceFile` is interned, so memoised
   * downstream queries (`def_map`, `typecheck_mapping`) invalidate
   * incrementally instead of falling off a cliff.
   */
  updateFile(handle: FileHandle, contents: string): void {
    this._inner.update_file(handle, contents);
  }

  /**
   * Close a file in the workspace. Strict in signal: closing an unknown /
   * already-closed handle throws so JS-side bugs surface loudly (mirrors
   * `ty_wasm`).
   */
  closeFile(handle: FileHandle): void {
    this._inner.close_file(handle);
  }

  /**
   * Workspace-wide diagnostic drain. Runs `parse → def_map →
   * typecheck_mapping` across every open file and returns a flat array of
   * {@link CheckRow}. The LSP Worker (07-03) republishes these grouped by URI
   * as `textDocument/publishDiagnostics` notifications.
   */
  check(): CheckRow[] {
    return this._inner.check() as CheckRow[];
  }

  /**
   * Per-file diagnostic drain — the B3 follow-up accessor the LSP Worker uses
   * for its per-file `publishDiagnostics` notifications. `check()` returns
   * the workspace-wide flat array; `diagnosticsFor` returns just one file's
   * rows so 07-03 can dispatch one notification per affected URI without
   * partitioning the workspace array on the JS side.
   */
  diagnosticsFor(handle: FileHandle): CheckRow[] {
    return this._inner.diagnostics_for(handle) as CheckRow[];
  }

  /**
   * Compile one open file. Returns `{ sql, manifest_yaml }` — same shape as
   * {@link compile}. Preferred over `compile(source)` for the run path because
   * it consumes the file's stable Salsa `SourceFile` identity (so subsequent
   * edits benefit from incremental memoisation).
   */
  compileFile(handle: FileHandle): CompileResult {
    return this._inner.compile_file(handle) as CompileResult;
  }

  /**
   * Install a user-supplied ShEx schema as the active output descriptor. On
   * parse failure the previously-installed descriptor is RETAINED (no
   * half-applied state — same contract as `fossil-lsp::load_sibling_shex` in
   * 06-09). Future `fossil-ide` feature calls (hover, completion) reading
   * `HirDb::output_descriptor_kind` see the new schema atomically.
   */
  setTargetShex(text: string): void {
    this._inner.set_target_shex(text);
  }

  /**
   * Register an {@link InferredDescriptorJson} for a source binding name
   * BEFORE invoking {@link compile} / {@link compileFile}. The Rust compiler
   * reads from this registration during forward type propagation (Phase 3
   * CORE-05 rewired in plan 13-02).
   *
   * The browser-side playground orchestration runs DuckDB-WASM
   * `DESCRIBE read_csv_auto('<resolved-url>')` for each `io.csv("...")`
   * reference in the source, canonicalises the columns to the
   * {@link InferredPrimitive} catalog, and calls this method with the
   * resulting descriptor before invoking {@link compile} or
   * {@link compileFile}. See ADR-0037 for the full architectural rationale.
   *
   * Keyed by source-binding name (e.g. `"users"` for
   * `users := io.csv("...")`), NOT by URL. Idempotent — re-registering with
   * the same `source_name` overwrites the previous entry.
   *
   * @throws Error if the descriptor JSON fails to deserialise on the Rust
   *         side (e.g. missing required fields). The error message includes
   *         the underlying serde_json diagnostic.
   *
   * @see ADR-0037 — drop user-facing CSVW; infer via DuckDB DESCRIBE
   */
  registerInferredDescriptor(descriptor: InferredDescriptorJson): void {
    // The wasm-bindgen wrapper exposes `registerInferredDescriptor(string)` —
    // it accepts a JSON string. Serialise here so callers pass a typed object.
    this._inner.registerInferredDescriptor(JSON.stringify(descriptor));
  }
}

// Re-export the Token + Diagnostic + Resolver types for ergonomics — consumers
// can import the full surface from `@fossil-lang/wasm` without also reaching
// for `@fossil-lang/types` (still works; this is convenience).
export type { TokenRow, SemanticTokensLegend } from '@fossil-lang/types';
