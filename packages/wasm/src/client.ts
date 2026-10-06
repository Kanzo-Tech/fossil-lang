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
 * Mirrors `@fossil-lang/corpus`'s `client.ts` split, the known-good shape.
 */
import {
  FossilWorkspace as RawFossilWorkspace,
  FileHandle as RawFileHandle,
  tokenize as rawTokenize,
  refs as rawRefs,
  providers as rawProviders,
} from '../pkg/fossil_wasm.js';
import {
  until,
  type CheckRow,
  type CompletionRow,
  type DefinitionRow,
  type DocumentWorkspace,
  type HoverRow,
  type InferredDescriptor,
  type MissingDocument,
  type Problem,
  type ProgramSource,
  type ProviderInfo,
  type SemanticTokenRow,
  type SourceRefInfo,
  type TokenRow,
} from '@fossil-lang/types';
import { initFossilWasm, type BootOptions } from './load.js';

/**
 * Opaque file-handle returned by {@link FossilWorkspace.openFile}. Pass it
 * back into the matching `updateFile` / `closeFile` / `diagnosticsFor`
 * calls. JS code cannot construct one directly (the wasm-bindgen class has a
 * private constructor) — handles are minted only by `openFile` on the Rust
 * side, where they index a `HashMap<FileHandle, SourceFile>` keyed by a `u32`.
 */
export type FileHandle = RawFileHandle;

/**
 * Tokenize a Fossil source string with the compiler's own lexer: each token by name, its offsets in
 * UTF-16 code units.
 *
 * MUST be called after {@link initFossilWasm} has resolved; otherwise the
 * underlying wasm-bindgen function throws (the wasm module is not yet
 * instantiated).
 */
export function tokenize(text: string): TokenRow[] {
  return rawTokenize(text);
}

/**
 * Parse a Fossil program and return its external references — the typed lineage
 * (every data URI + `schema =` argument, each tagged with its `@conn` alias and
 * role). A host reads this to derive a program's connections WITHOUT a compile
 * or a server round-trip. Identical shape to the native `fossil_lineage::SourceRefInfo`,
 * so every host reads the same answer.
 *
 * **It boots the module itself**, the way {@link openProgram} does: the boot is shared and kept,
 * so the call after the first costs the parse and nothing else, and a host writes no
 * `initFossilWasm()` before it. That is why it answers a promise where {@link tokenize} does not —
 * the lexer runs on every keystroke inside an editor that has already booted, and this runs once
 * per question.
 */
export async function refs(program: string, options: BootOptions = {}): Promise<SourceRefInfo[]> {
  await until(initFossilWasm(options.wasm), options.signal);
  return rawRefs(program);
}

/**
 * List the data-source providers fossil supports (`io.csv`, `io.rdf`, …) — the
 * provider name, the extensions it reads, and how it can be used. Compiled into the module,
 * so a host may keep the answer for as long as it keeps the module.
 *
 * Boots the module itself, for the reason {@link refs} does. {@link providerFor} is the
 * question a host usually asks of the answer.
 */
export async function providers(options: BootOptions = {}): Promise<ProviderInfo[]> {
  await until(initFossilWasm(options.wasm), options.signal);
  return rawProviders();
}

/**
 * Workspace API class. Thin TS wrapper around the wasm-bindgen
 * `FossilWorkspace` that exposes camelCase method names for JS idiom + better
 * TS inference (the raw bindings use snake_case from the Rust impl block).
 *
 * One instance per browser tab / Node process — the instance owns the Salsa
 * store + `Arc<OutputDescriptorKind>` for the schema slot.
 *
 * NOTE: {@link FileHandle} is opaque — JS cannot construct one. Callers receive
 * a handle from `openFile` and pass it back to subsequent operations.
 */
export class FossilWorkspace {
  private _inner: RawFossilWorkspace;

  constructor() {
    this._inner = new RawFossilWorkspace();
  }

  /**
   * Free the underlying WASM-side Salsa store. Call when the workspace is no
   * longer needed (e.g. component unmount).
   */
  free(): void {
    this._inner.free();
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
   * `Setter` (`set_text`) — bumps the revision for incremental
   * invalidation rather than a full recompute, and that revision bump is also
   * what cancels any analysis still running on an older snapshot.
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
   * The connection map `@name/…` expands against — what
   * `Host.connections()` answers. It reaches locators only, so setting it
   * re-checks nothing.
   */
  setConnections(connections: Record<string, string>): void {
    this._inner.setConnections(connections);
  }

  /**
   * The documents the file at `handle` names and nothing has registered, each
   * with the key to register it under and the locator to read it from.
   */
  missingDocuments(handle: FileHandle): MissingDocument[] {
    return this._inner.missingDocuments(handle);
  }

  /** Register a fetched document's text under the key {@link missingDocuments} reported. */
  registerDocument(key: string, text: string): void {
    this._inner.registerDocument(key, text);
  }

  /**
   * The data sources the file at `handle` reads, as fossil resolved them: the
   * binding, the URI as written, its locator through the connection map, the
   * catalogue row and the reader option.
   */
  sources(handle: FileHandle): ProgramSource[] {
    return this._inner.sources(handle);
  }

  /**
   * The file at `handle` as a {@link DocumentWorkspace}, for
   * `resolveDocuments(workspace.workspace(handle), host)`.
   */
  workspace(handle: FileHandle): DocumentWorkspace {
    return {
      setConnections: (connections) => this.setConnections(connections),
      missingDocuments: () => this.missingDocuments(handle),
      registerDocument: (key, text) => this.registerDocument(key, text),
    };
  }

  /**
   * Workspace-wide diagnostic drain. Runs `parse → def_map → typecheck_mapping`
   * across every open file and returns a flat array of {@link CheckRow}.
   */
  check(): CheckRow[] {
    return this._inner.check();
  }

  /**
   * Per-file diagnostic drain: one file's rows of {@link check}.
   */
  diagnosticsFor(handle: FileHandle): CheckRow[] {
    return this._inner.diagnostics_for(handle);
  }

  /**
   * What is under the cursor — `{ markdown, range }`, or `null` when nothing
   * there has a type.
   *
   * `line` / `character` are LSP: zero-based, `character` in UTF-16 code units.
   * A CodeMirror or Monaco host already counts in those units, so a document
   * offset converts with `doc.lineAt(pos)` and no byte arithmetic.
   *
   * ## Push the buffer before you ask
   *
   * This reads the text of the last {@link updateFile}. Hover fires on
   * mouse-move and the checker is debounced, so a hover mid-debounce answers
   * about text one keystroke old and its range lands one keystroke wrong. The
   * three read-only methods take a SHARED borrow on the Rust side and cannot
   * poison the workspace the way a re-entered `updateFile` once could — see the
   * `ide` module in `crates/fossil-wasm` — but staleness is not a borrow
   * problem and nothing here can fix it for you.
   */
  hover(handle: FileHandle, line: number, character: number): HoverRow | null {
    // `serde_wasm_bindgen` writes `None` as `undefined`; a host reading this
    // should have one falsy answer to check, not two.
    return this._inner.hover(handle, line, character) ?? null;
  }

  /**
   * The completion candidates at a position, already narrowed by the receiver:
   * `str.` offers string members and no reader, a property-key position offers
   * the target shape's predicates and no catalogue row at all.
   *
   * `kind` is the LSP `CompletionItemKind` **by name** (`"function"`,
   * `"field"`) rather than by number. The numbers never cross this boundary —
   * `packages/codemirror-fossil`'s deleted predecessor is what happens when
   * they do.
   *
   * The same staleness note as {@link hover} applies, and harder: completion
   * fires on nearly every keystroke.
   */
  completions(handle: FileHandle, line: number, character: number): CompletionRow[] {
    return this._inner.completions(handle, line, character);
  }

  /**
   * Where the name under the cursor is defined. Empty when nothing there has a
   * definition.
   *
   * `uri` is the key the buffer was opened under, verbatim — and two of the
   * four positions this recognises resolve into the **shape document**, so a
   * host with a single editor pane has to read `uri` before it moves a cursor.
   */
  gotoDefinition(handle: FileHandle, line: number, character: number): DefinitionRow[] {
    return this._inner.gotoDefinition(handle, line, character);
  }

  /**
   * Every classified span of the file at `handle`, in source order — the
   * semantic layer an editor lays over {@link tokenize}'s lexical one. Ranges
   * are LSP (UTF-16), like {@link hover}'s, and the same staleness note
   * applies: push the buffer before you ask.
   */
  semanticTokens(handle: FileHandle): SemanticTokenRow[] {
    return this._inner.semanticTokens(handle);
  }

  /**
   * Register an {@link InferredDescriptor} under the source URI the program
   * wrote, BEFORE invoking {@link check}. The Rust compiler reads from this
   * during forward type propagation.
   *
   * The compiler never introspects a source itself: it performs no network or
   * file IO — that would break the WASM gate and Salsa's determinism alike —
   * so a host that wants column types must run the `DESCRIBE` and push the
   * result in here.
   *
   * @throws {FossilError} `api/invalid-argument` if the descriptor fails to deserialise on the Rust
   *   side, the `serde_json` error as its cause.
   */
  registerInferredDescriptor(descriptor: InferredDescriptor): void {
    // The wasm-bindgen wrapper accepts a JSON string; serialise here so callers
    // pass a typed object.
    this._inner.registerInferredDescriptor(JSON.stringify(descriptor));
  }

  /**
   * Record that the host could not describe the source the program wrote as `key`, and why. A
   * check then reports it at the binding that reads `key` — one warning per key, under
   * `problem.code` — until a descriptor is registered under the same key, which forgets it.
   *
   * @throws {FossilError} `api/invalid-argument` if `problem` carries a code this build of fossil
   *   does not know, the `serde_json` error as its cause.
   */
  registerUndescribed(key: string, problem: Problem): void {
    this._inner.registerUndescribed(key, JSON.stringify(problem));
  }
}
