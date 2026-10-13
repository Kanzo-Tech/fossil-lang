/**
 * @fossil-lang/wasm — the fossil compiler front end in the browser, over the `fossil-wasm`
 * wasm-bindgen artefacts.
 *
 * - {@link openProgram} — one program open for an editor: the incremental workspace, the documents
 *   it names read through the host, and the position queries `@fossil-lang/codemirror-fossil`'s
 *   `fossil(program)` draws from.
 * - {@link inputs} — what a program reads, with no editor: the `ts.preProcessFile` of fossil.
 * - {@link formats} and {@link formatFor} — the formats this build reads, and which reads a file.
 * - {@link runProfiler} — `fossil-profile`'s plan over some sources, each query run on the host's
 *   engine: what `@fossil-lang/profile` describes a program's sources with.
 * - {@link grammarFor} — the grammar of Fossil for a request's names: what a constrained decoder
 *   holds a model to, every name in it one that exists.
 * - {@link initFossilWasm} — Node only: hand over the `.wasm` bytes. Every door boots itself.
 *
 *   const program = await openProgram('job.fossil', { host, text });
 *   const extensions = fossil(program, { onNavigate });
 */

// The wasm-bindgen glue (`../pkg/fossil_wasm.js`) is imported ONLY from leaf modules — `load.ts`,
// `program.ts`, `format.ts`, `profile.ts` and `grammar.ts` — never here in the entry: importing the stateful glue from the
// package entry lets a `sideEffects:false` bundler duplicate it, splitting `init()`'s wasm instance
// from the one the functions use (the crash the codemirror tokenizer hit on the main thread).
export { initFossilWasm, type InitInput } from './load.js';
export { formatFor, formats } from './format.js';
export { inputs, openProgram } from './program.js';
export type { FossilProgram, OpenProgramOptions } from './program.js';
export { runProfiler, type ProfileRows } from './profile.js';
export { grammarFor, type Grammar } from './grammar.js';
