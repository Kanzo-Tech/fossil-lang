/**
 * Half one of the loop: type-check as you type.
 *
 * `@fossil-lang/wasm` is the LSP server-side in the browser — the same `fossil-ide` free
 * functions the native `fossil-lsp` drives over stdio, driven here over a function call.
 * `FossilPlayground` is a workspace, not a compiler: it holds open files and re-checks
 * them, because an editor edits.
 *
 * ## Two things a host has to supply, and neither is optional
 *
 * 1. **The shape document, by opening it.** `hello.fossil` says `io.shex("hello.shex")`.
 *    The playground's filesystem is `WasmSystem`'s in-memory map and it is EMPTY — a
 *    document the host never opened is not there, and `fossil-wasm` says so in as many
 *    words: *"The playground's way to give the compiler a shape document is to open it."*
 *    So `open()` opens the `.shex` FIRST. Without it the `name` key resolves against
 *    nothing, the mapping writes no properties, and the run still succeeds with the
 *    column simply absent — which is the failure mode worth knowing about, because it is
 *    silent.
 * 2. **The input columns, by registering a descriptor.** `User := io.csv("users.csv")`
 *    declares no columns; they are introspected from the real file. Natively
 *    `fossil-introspect` runs a `DESCRIBE` through DuckDB. Here `descriptor.ts` does the
 *    same through DuckDB-WASM and pushes the answer in with
 *    `registerInferredDescriptor` BEFORE the check.
 *
 * Skip (2) and `User.name` is an unknown column: the check reports it, correctly, and the
 * editor looks broken for a program that is fine.
 */
import {
  FossilPlayground,
  initFossilWasm,
  tokenize,
  tokenKinds,
  type CheckRow,
  type CompletionRow,
  type DefinitionRow,
  type FileHandle,
  type HoverRow,
  type InferredDescriptorJson,
} from '@fossil-lang/wasm';
import wasmUrl from '@fossil-lang/wasm/pkg/fossil_wasm_bg.wasm?url';

import { PROGRAM_PATH, SHEX, SHEX_PATH } from './example.js';

export type { CheckRow, CompletionRow, DefinitionRow, HoverRow };

/** The lexer and its legend, re-exported so `App` hands the editor one module.
 *  `tokenKinds()` is what makes `TokenRow.kind` readable — see the legend note in
 *  `@fossil-lang/codemirror-fossil`'s `tags.ts` for the nine-variant drift that
 *  happened the last time a consumer wrote the numbers down. */
export { tokenize, tokenKinds };

/** What `load()` measured on the way in — the cost of the checker, reported not guessed. */
export interface BundleCost {
  /** Bytes over the wire for the `.wasm`, as the browser reports it. */
  bytes: number;
  /** Wall-clock milliseconds from first byte requested to instantiated and callable. */
  ms: number;
}

let playground: FossilPlayground | null = null;
let programHandle: FileHandle | null = null;
let cost: BundleCost | null = null;
/** The text last pushed into the workspace. See {@link sync}. */
let pushed: string | null = null;

/** The measured cost of the checker bundle, or `null` before {@link load}. */
export function checkerCost(): BundleCost | null {
  return cost;
}

/**
 * Fetch + instantiate the checker, open the shape document, open the program.
 *
 * Measured rather than declared: the `.wasm` is fetched here as a `Response` so its
 * `Content-Length` is readable, and `initFossilWasm` accepts one directly — wasm-bindgen's
 * `--target web` init takes `module_or_path`, and a `Response` is the streaming path.
 */
export async function load(program: string): Promise<void> {
  if (playground) return;
  const started = performance.now();
  const response = await fetch(wasmUrl);
  const buffer = await response.arrayBuffer();
  await initFossilWasm({ wasmUrl: new Response(buffer, { headers: { 'content-type': 'application/wasm' } }) });
  cost = { bytes: buffer.byteLength, ms: Math.round(performance.now() - started) };

  playground = new FossilPlayground();
  // The shape document first: opening it is what puts it in the registry, and the
  // registry is a Salsa input, so opening it AFTER the program would also work (every
  // query that missed it re-executes). Doing it first just means the first check is right.
  playground.openFile(SHEX_PATH, SHEX);
  programHandle = playground.openFile(PROGRAM_PATH, program);
  pushed = program;
}

/**
 * Push the buffer into the workspace, unless it is already there.
 *
 * **Four callers at four rates share one workspace, and this is what makes that
 * safe to read as well as safe to call.** The linter runs on a 120 ms debounce;
 * hover fires when the pointer rests; completion fires on nearly every
 * keystroke; goto-def fires on a key. All four answer about the text of the last
 * `updateFile`, so all four push first — otherwise the three fast ones would be
 * answering about text one keystroke old, and a hover range one character off is
 * the sort of wrong that reads as an editor bug.
 *
 * The string comparison is what makes that cheap. `updateFile` is the one method
 * that takes the workspace's EXCLUSIVE borrow and the one that bumps the Salsa
 * revision, so calling it per mouse-move would invalidate the memoised check the
 * squiggles came from for no reason at all. In the common case — the pointer
 * moving over text nobody has touched since the last check — this compares two
 * strings and returns.
 *
 * A second workspace for the position queries was the alternative, and it is
 * worse: double the interning and double the memory, to answer from a different
 * revision than the diagnostics on screen. The re-entrancy that made sharing look
 * dangerous is fixed at the root — the three position methods take a SHARED
 * borrow on the Rust side because none of them mutates.
 */
function sync(text: string): void {
  if (!playground || programHandle === null || text === pushed) return;
  playground.updateFile(programHandle, text);
  pushed = text;
}

/** Push a host-introspected input schema at the compiler. See `descriptor.ts`. */
export function registerDescriptor(descriptor: InferredDescriptorJson): void {
  playground?.registerInferredDescriptor(descriptor);
}

/**
 * Re-check after an edit — the `textDocument/didChange` path, and the same Salsa setter.
 *
 * **The guard that used to be here is gone, and both halves of why it was here are
 * fixed.** `updateFile` could be re-entered while a previous call was still on the
 * stack, and wasm-bindgen's exclusive borrow of the exported object does not fail
 * gracefully — it panics, "recursive use of an object detected which would lead to
 * unsafe aliasing in rust", and on wasm32 a panic is an abort, so the borrow flag
 * it held is never cleared and every later call fails identically. The workspace
 * stayed poisoned for the rest of the session. This module carried a `busy` flag
 * and `App` a 120 ms `setTimeout` to stay clear of it.
 *
 * `crates/fossil-wasm` no longer takes that borrow: the exported class holds the
 * workspace in its own `RefCell` and every method takes `&self`, so re-entry
 * RETURNS a catchable error naming the method rather than aborting, and the
 * workspace still works afterwards.
 *
 * And the coalescing moved to where a host cannot forget it —
 * `@fossil-lang/codemirror-fossil`'s linter waits out its `delay` AND waits for
 * this promise before scheduling again, which is what an LSP client does with
 * `didChange` anyway. This function is now what it always should have been: one
 * edit, one check, no scheduling of its own.
 */
export function checkText(program: string): CheckRow[] {
  if (!playground || programHandle === null) return [];
  sync(program);
  return playground.check();
}

/**
 * The type under the cursor, and the type the shape demands of it.
 *
 * The three functions below are the whole of the LSP surface the tab was
 * missing: `fossil-ide` has had hover, completion and goto-def all along and
 * `lsp_worker.rs` dispatches them, but only over `postMessage` from a Worker —
 * which needs an LSP client on the other end. `crates/fossil-wasm`'s `ide`
 * module puts the same three answers on the main thread as method calls, and
 * these three lines are what that buys.
 */
export function hoverAt(text: string, line: number, character: number): HoverRow | null {
  if (!playground || programHandle === null) return null;
  sync(text);
  return playground.hover(programHandle, line, character);
}

/** The candidates at the cursor, narrowed by the receiver's type. */
export function completeAt(text: string, line: number, character: number): CompletionRow[] {
  if (!playground || programHandle === null) return [];
  sync(text);
  return playground.completions(programHandle, line, character);
}

/** Where the name under the cursor is defined — often in `hello.shex`. */
export function definitionAt(text: string, line: number, character: number): DefinitionRow[] {
  if (!playground || programHandle === null) return [];
  sync(text);
  return playground.gotoDefinition(programHandle, line, character);
}

/** Check without editing — used once after the descriptor lands. */
export function check(): CheckRow[] {
  return playground?.check() ?? [];
}

/** LSP severity 1 is an error; 2 a warning. A program with no 1s is runnable. */
export function hasErrors(rows: readonly CheckRow[]): boolean {
  return rows.some((row) => row.severity === 1);
}
