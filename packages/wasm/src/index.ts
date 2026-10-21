/**
 * @fossil-lang/wasm — the fossil compiler front end in the browser, over the `fossil-wasm`
 * wasm-bindgen artefacts.
 *
 * - {@link openProgram} — one program open for an editor: the incremental workspace, the documents
 *   it names read through the host, and the position queries `@fossil-lang/codemirror-fossil`'s
 *   `fossil(program)` draws from.
 * - {@link inputs} — what a program reads, with no editor: the `ts.preProcessFile` of fossil.
 * - {@link formats} and {@link formatFor} — the formats this build reads, and which reads a file.
 * - {@link profile} — what some sources are, on the host's engine, before there is a program;
 *   `program.describe` is the same over a program's own inputs. {@link runProfiler} is its loop.
 * - {@link infer} — the model a profile implies, and each question it took a default for.
 * - {@link write} — the program a model implies: what a host decided a graph holds, as Fossil;
 *   `program.mapping()` is the RML mapping its types describe, before any run.
 * - {@link DUCKDB_TYPES} — what a DuckDB `DESCRIBE` column type means on the lattice.
 * - {@link bindingName} — the binding name a file becomes in a program, one `IDENT`.
 * - {@link initFossilWasm} — Node only: hand over the `.wasm` bytes. Every door boots itself.
 *
 *   const program = await openProgram('job.fossil', { host, text });
 *   const extensions = fossil(program, { onNavigate });
 *   const profiled = await program.describe({ engine, options: { depth: 'types' } });
 */

// The wasm-bindgen glue (`../pkg/fossil_wasm.js`) is imported ONLY from leaf modules — `load.ts`,
// `program.ts`, `format.ts`, `profile.ts`, `name.ts`, `infer.ts` and `write.ts` — never here in the
// entry: importing the stateful glue from the
// package entry lets a `sideEffects:false` bundler duplicate it, splitting `init()`'s wasm instance
// from the one the functions use (the crash the codemirror tokenizer hit on the main thread).
export { initFossilWasm, type InitInput } from './load.js';
export { formatFor, formats } from './format.js';
export { inputs, openProgram } from './program.js';
export type { FossilProgram, OpenProgramOptions } from './program.js';
export { runProfiler, type ProfileRows } from './profile.js';
export { profile, type ProfileIO } from './describe.js';
export { bindingName } from './name.js';
export { infer } from './infer.js';
export { write } from './write.js';
export { DUCKDB_TYPES, type DuckdbType } from './catalogue.generated.js';
