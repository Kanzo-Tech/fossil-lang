/**
 * @fossil-lang/wasm — JS/TS wrapper around the fossil-wasm wasm-bindgen artifacts.
 *
 * Public API (one of the @fossil-lang/* packages; `git ls-files packages` is
 * the list):
 * - {@link initFossilWasm} — boots the module; its `.wasm` is a bundler asset. A failed boot is
 *   forgotten, so the next call tries again.
 * - {@link tokenize} — calls the Rust lexer, returns TokenRow[].
 * - {@link openProgram} — one program open for an editor: the workspace, the push-before-ask
 *   discipline and the document resolution a host would otherwise write, shaped to spread into
 *   `@fossil-lang/codemirror-fossil`'s `fossil()`. What a host with one editor calls.
 * - {@link refs} and {@link providers} — a program's external references, and the providers this
 *   build reads. Each boots the module itself, as `openProgram` does: no `initFossilWasm()` first.
 * - {@link providerFor} — which of those providers reads a path, as data or as a schema.
 * - {@link FossilWorkspace} — Workspace API class: open / update / close,
 *   `check`, the three position queries (`hover`, `completions`,
 *   `gotoDefinition`) and `semanticTokens` an editor draws its IDE surface from, and the documents
 *   and sources a program reads (`missingDocuments`, `registerDocument`,
 *   `sources`) that a host resolves through its `Host`.
 *
 * Consumer pattern (wasm-bindgen --target web):
 *
 *   import { openProgram } from '@fossil-lang/wasm';
 *   import { fossil } from '@fossil-lang/codemirror-fossil';
 *
 *   const program = await openProgram('job.fossil', { host, text });
 *   const extensions = fossil({ ...program, onNavigate });
 */

// The wasm-bindgen glue (`../pkg/fossil_wasm.js`) is imported ONLY from leaf
// modules — `load.ts` (init) and `client.ts` (the lexer / LSP / Workspace API) —
// never here in the entry. See `client.ts`: importing the stateful glue from the
// package entry lets a `sideEffects:false` bundler duplicate it, splitting
// `init()`'s wasm instance from the one the functions use (the crash the
// codemirror tokenizer hit on the main thread). This entry only re-exports.
export { initFossilWasm } from './load.js';
export type { BootOptions, InitInput } from './load.js';

export {
  tokenize,
  FossilWorkspace,
  refs,
  providers,
} from './client.js';
export type { FileHandle } from './client.js';

export { providerFor } from './provider.js';

export { openProgram } from './program.js';
export type { FossilProgram, Introspection, OpenProgramOptions } from './program.js';
