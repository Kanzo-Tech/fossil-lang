/**
 * The wasm-bindgen-backed runtime surface (lexer, LSP worker, Workspace API) of
 * @fossil-lang/wasm.
 *
 * Kept in its OWN module (NOT the package entry) so the wasm-bindgen glue
 * (`../pkg/fossil_wasm.js`) is imported only from leaf modules — `load.ts` (for
 * `init`) and here. Under a bundler with `"sideEffects": false`, importing the
 * stateful glue from the entry chunk can duplicate it: `init()` then populates
 * the `wasm` binding in one instance while these functions read `undefined` from
 * another (→ `Cannot read properties of undefined (reading '__wbindgen_malloc…')`,
 * seen when the codemirror tokenizer calls `tokenize` on the main thread).
 * Mirrors `@fossil-lang/graph`'s `client.ts` split, the known-good shape.
 */
import {
  FossilPlayground as RawFossilPlayground,
  FileHandle as RawFileHandle,
  tokenize as rawTokenize,
  semantic_legend as rawSemanticLegend,
  start_lsp_worker as rawStartLspWorker,
  refs as rawRefs,
  providers as rawProviders,
} from '../pkg/fossil_wasm.js';
import type { TokenRow, SemanticTokensLegend } from '@fossil-lang/types';
import type {
  CheckRow,
  StdlibClass,
  InferredDescriptorJson,
  SourceRefInfo,
  ProviderInfo,
} from './index.js';

/**
 * Install the LSP-over-postMessage dispatcher on the current Worker scope.
 *
 * Per ADR-0024 (`fossil-wasm` IS the LSP server-side) + Phase 7 plan 07-03
 * (the 16-route dispatch loop). The Rust function (re-exported from
 * `crates/fossil-wasm/src/lsp_worker.rs`) installs `self.onmessage` on the
 * Worker scope and owns LSP JSON-RPC dispatch from that point forward.
 *
 * MUST be called inside a Web Worker scope, AFTER {@link initFossilWasm} has
 * resolved. Calling it on the main thread is a no-op (the dispatcher needs
 * `DedicatedWorkerGlobalScope.onmessage`).
 */
export function start_lsp_worker(): void {
  rawStartLspWorker();
}

/**
 * Opaque file-handle returned by {@link FossilPlayground.openFile}. Pass it
 * back into the matching `updateFile` / `closeFile` / `diagnosticsFor`
 * calls. JS code cannot construct one directly (the wasm-bindgen class has a
 * private constructor) — handles are minted only by `openFile` on the Rust
 * side, where they index a `HashMap<FileHandle, SourceFile>` keyed by a `u32`.
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
 * categories. Re-export of `fossil_ide::semantic_legend` over the wasm-bindgen
 * boundary — NO duplicate legend definition lives here.
 *
 * MUST be called after {@link initFossilWasm} has resolved.
 */
export function semanticLegend(): SemanticTokensLegend {
  return rawSemanticLegend() as SemanticTokensLegend;
}

/**
 * Parse a Fossil program and return its external references — the typed lineage
 * (every data URI + `schema =` argument, each tagged with its `@conn` alias and
 * role). keasy's client-compute job runner reads this to derive a job's
 * connections WITHOUT subprocessing `fossil` / a server round-trip. Identical
 * shape to the native `fossil refs` (the SAME `fossil_run_status::SourceRefInfo`
 * struct), so the browser and the CLI never diverge.
 *
 * MUST be called after {@link initFossilWasm} has resolved.
 */
export function refs(program: string): SourceRefInfo[] {
  return rawRefs(program) as SourceRefInfo[];
}

/**
 * List the data-source providers fossil supports (`io.csv`, `io.rdf`, …) — the
 * provider name, the extensions it reads, and how it can be used. Identical
 * shape to the native `fossil providers`.
 *
 * MUST be called after {@link initFossilWasm} has resolved.
 */
export function providers(): ProviderInfo[] {
  return rawProviders() as ProviderInfo[];
}

/**
 * Workspace API class (ADR-0024). Thin TS wrapper around the wasm-bindgen
 * `FossilPlayground` that exposes camelCase method names for JS idiom + better
 * TS inference (the raw bindings use snake_case from the Rust impl block).
 *
 * One instance per browser tab / Node process — the instance owns the Salsa
 * store + `Arc<OutputDescriptorKind>` for the schema slot.
 *
 * NOTE: {@link FileHandle} is opaque — JS cannot construct one. Callers receive
 * a handle from `openFile` and pass it back to subsequent operations.
 */
export class FossilPlayground {
  private _inner: RawFossilPlayground;

  constructor() {
    this._inner = new RawFossilPlayground();
  }

  /**
   * Free the underlying WASM-side Salsa store. Call when the playground is no
   * longer needed (e.g. component unmount).
   */
  free(): void {
    this._inner.free();
  }

  /**
   * Return the stdlib classification manifest (STDL-07). The playground reads
   * this once at startup to render `native_udf_only` functions as disabled.
   */
  classification(): StdlibClass[] {
    return this._inner.classification() as StdlibClass[];
  }

  /**
   * Open a file in the workspace. Returns the {@link FileHandle} subsequent
   * `updateFile` / `closeFile` / `diagnosticsFor` calls key on.
   */
  openFile(path: string, contents: string): FileHandle {
    return this._inner.open_file(path, contents);
  }

  /**
   * Apply an edit to an open file. Mutates the SAME `SourceFile` via the Salsa
   * `Setter` (`set_text`) — bumps the revision (ADR-0022) for incremental
   * invalidation rather than a full recompute.
   */
  updateFile(handle: FileHandle, contents: string): void {
    this._inner.update_file(handle, contents);
  }

  /**
   * Close a file in the workspace. Strict: closing an unknown / already-closed
   * handle throws so JS-side bugs surface loudly.
   */
  closeFile(handle: FileHandle): void {
    this._inner.close_file(handle);
  }

  /**
   * Workspace-wide diagnostic drain. Runs `parse → def_map → typecheck_mapping`
   * across every open file and returns a flat array of {@link CheckRow}.
   */
  check(): CheckRow[] {
    return this._inner.check() as CheckRow[];
  }

  /**
   * Per-file diagnostic drain — the accessor the LSP Worker uses for its
   * per-file `publishDiagnostics` notifications.
   */
  diagnosticsFor(handle: FileHandle): CheckRow[] {
    return this._inner.diagnostics_for(handle) as CheckRow[];
  }

  /**
   * Install a user-supplied ShEx schema as the active output descriptor. On
   * parse failure the previously-installed descriptor is RETAINED.
   */
  setTargetShex(text: string): void {
    this._inner.set_target_shex(text);
  }

  /**
   * Register an {@link InferredDescriptorJson} for a source binding name BEFORE
   * invoking {@link check}. The Rust compiler reads from this during forward
   * type propagation.
   *
   * @throws Error if the descriptor JSON fails to deserialise on the Rust side.
   * @see ADR-0037
   */
  registerInferredDescriptor(descriptor: InferredDescriptorJson): void {
    // The wasm-bindgen wrapper accepts a JSON string; serialise here so callers
    // pass a typed object.
    this._inner.registerInferredDescriptor(JSON.stringify(descriptor));
  }
}
