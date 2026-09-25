/**
 * Half one of the loop: type-check as you type.
 *
 * `@fossil-lang/wasm` is the LSP server-side in the browser — the same `fossil-ide` free
 * functions the native `fossil-lsp` drives over stdio, driven here over a function call.
 * `openProgram` is the whole of the session: one workspace, the text pushed before every
 * answer, the documents it names resolved before a check, and the answer shaped for
 * `@fossil-lang/codemirror-fossil`'s `fossil()`. This file used to BE that protocol —
 * two hundred lines, and keasy carried a copy of them — and now it only measures the boot.
 *
 * ## Two things a host has to supply, and neither is optional
 *
 * 1. **The shape document, through a `SourceHost`.** `hello.fossil` says
 *    `io.shex("hello.shex")`. The checker reads nothing itself: it reports the document
 *    missing, `resolveDocuments` has {@link HOST} sign its locator and fetches it, and
 *    the text is registered under the key the program wrote. Without it the `name` key
 *    resolves against nothing, the mapping writes no properties, and the run still
 *    succeeds with the column simply absent — which is the failure mode worth knowing
 *    about, because it is silent.
 * 2. **The input columns, by registering a descriptor.** `User := io.csv("users.csv")`
 *    declares no columns; they are introspected from the real file. Natively
 *    `fossil-introspect` runs a `DESCRIBE` through DuckDB. Here `descriptor.ts` does the
 *    same through DuckDB-WASM and pushes the answer in with
 *    `registerInferredDescriptor` BEFORE the check.
 *
 * Skip (2) and `User.name` is an unknown column: the check reports it, correctly, and the
 * editor looks broken for a program that is fine.
 */
import { openProgram, type CheckRow, type FossilProgram } from '@fossil-lang/wasm';

import { HOST, PROGRAM_PATH } from './example.js';

export type { CheckRow, FossilProgram };

/** What `load()` measured on the way in — the cost of the checker, reported not guessed. */
export interface BundleCost {
  /** Bytes over the wire for the `.wasm`, as the browser reports it. */
  bytes: number;
  /** Wall-clock milliseconds from first byte requested to instantiated and callable. */
  ms: number;
}

let cost: BundleCost | null = null;

/**
 * What the browser fetched for the `.wasm` whose file name starts with `stem`, since `started`.
 *
 * Read off Resource Timing rather than a `fetch` of our own: the module locates its `.wasm`
 * itself (`new URL(…, import.meta.url)`, emitted by Vite as `assets/<stem>-<hash>.wasm`), so
 * the app does not hold the bytes — it only observes the request the glue made.
 */
export function measured(stem: string, started: number): BundleCost {
  const entry = performance
    .getEntriesByType('resource')
    .filter((e): e is PerformanceResourceTiming => e.name.includes(stem) && /\.wasm(\?|$)/.test(e.name))
    .at(-1);
  return { bytes: entry?.decodedBodySize ?? 0, ms: Math.round(performance.now() - started) };
}

/** The measured cost of the checker bundle, or `null` before {@link load}. */
export function checkerCost(): BundleCost | null {
  return cost;
}

/**
 * Fetch + instantiate the checker, open the program, read the documents it names.
 *
 * Measured rather than declared: {@link measured} reads the request the module made for its
 * own `.wasm` — `openProgram` boots it with nothing, because the bundler emitted that file.
 */
export async function load(program: string): Promise<FossilProgram> {
  const started = performance.now();
  const opened = await openProgram(PROGRAM_PATH, { host: HOST, text: program });
  cost = measured('fossil_wasm_bg', started);
  return opened;
}

/** LSP severity 1 is an error; 2 a warning. A program with no 1s is runnable. */
export function hasErrors(rows: readonly CheckRow[]): boolean {
  return rows.some((row) => row.severity === 1);
}
