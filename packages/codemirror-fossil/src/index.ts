/**
 * `@fossil-lang/codemirror-fossil` — the fossil language layer for CodeMirror 6.
 *
 * Two halves, and they are the two halves `fossil-wasm` exposes:
 *
 * - **Highlighting**, from `tokenize()` + `tokenKinds()` — the compiler's own
 *   lexer, so no editor reimplements the grammar in TypeScript and drifts.
 * - **Diagnostics**, from `check()` — `CheckRow`s with UTF-16 spans, projected
 *   onto `@codemirror/lint`.
 *
 * ## It composes into an editor rather than being one
 *
 * Everything here is an `Extension`. There is no component, no `EditorView`
 * construction, no theme. `@kanzo-tech/ui`'s `CodeEditor` holds exactly this in a
 * `Compartment` and reconfigures it live, and so can anything else — the point of
 * a language layer is that the editor is somebody else's decision.
 *
 * ```ts
 * import { fossil } from '@fossil-lang/codemirror-fossil';
 * import { initFossilWasm, tokenize, tokenKinds, FossilPlayground } from '@fossil-lang/wasm';
 *
 * await initFossilWasm({ wasmUrl });
 * const pg = new FossilPlayground();
 * const handle = pg.openFile('hello.fossil', program);
 *
 * const extensions = fossil({
 *   tokenize,
 *   tokenKinds,
 *   uri: 'hello.fossil',
 *   check: (text) => { pg.updateFile(handle, text); return pg.check(); },
 * });
 * ```
 *
 * ## What it does not cover, and where that work already is
 *
 * **Hover, completion and goto-definition.** `crates/fossil-ide` has all three and
 * `crates/fossil-wasm`'s `lsp_worker.rs` dispatches sixteen LSP routes to them
 * over `postMessage` — but that is a Worker protocol, and the only functions on
 * the main-thread surface are `tokenize`, `tokenKinds`, `semanticLegend`, `check`,
 * `diagnosticsFor`, `refs` and `providers`. Wiring an LSP client to that Worker is
 * a real piece of work with its own shape (a `@codemirror/autocomplete` source and
 * a `hoverTooltip` over JSON-RPC), and stubbing it here against the main thread
 * would build the wrong thing. The deleted predecessor of this package shipped an
 * `autocomplete.ts` that completed `@`-prefixed connection aliases out of a
 * resolver — a host concern, not a language one, and it went with the package.
 *
 * **Semantic highlighting.** `semanticLegend()` is exported and `fossil-ide` knows
 * a type from a binding from a column; `tokenize()` does not, which is why `Ident`
 * carries no tag in `tags.ts`. The legend is on the main thread but the tokens
 * themselves only come back over the Worker, so this is the same gap as above.
 *
 * **Indentation.** Fossil is INDENT/DEDENT (`grammar.bnf`), so an `indentService`
 * would be genuinely useful and genuinely non-trivial — it needs the parser's view
 * of block openers, not the lexer's. Nothing here guesses at it.
 */
export { fossilHighlighting, buildDecorations } from './highlight.js';
export type { TokenSource, HighlightOptions } from './highlight.js';

export { fossilLinter, toDiagnostics } from './lint.js';
export type { CheckRowLike, CheckSource, LinterOptions } from './lint.js';

export { TAG_BY_NAME, tagFor } from './tags.js';
export { byteToUtf16Mapper } from './offsets.js';

import { type Extension } from '@codemirror/state';

import { fossilHighlighting, type HighlightOptions, type TokenSource } from './highlight.js';
import { fossilLinter, type CheckSource, type LinterOptions } from './lint.js';

/** Everything {@link fossil} needs, in one object. */
export interface FossilOptions extends TokenSource, HighlightOptions, Omit<LinterOptions, 'uri'> {
  /** The URI the buffer is open under in the workspace — the key `CheckRow.uri`
   *  carries, so rows for the OTHER open files (the `.shex` shape document, a
   *  second program) land on the right buffer. */
  uri: string;
  /** Run a check over the current text and return the workspace's rows.
   *
   *  The natural implementation updates the file and drains: `pg.updateFile(h,
   *  text); return pg.check();`. Doing both here rather than in two extensions is
   *  deliberate — the edit and the check are one step, and splitting them is how a
   *  host ends up checking text it has not yet pushed. */
  check: CheckSource;
}

/**
 * Both halves, as one extension.
 *
 * Omit `check` and you get highlighting alone, which is what a read-only view
 * wants — but the type requires it, because an editor without diagnostics is the
 * thing this package exists to stop being the only option.
 */
export function fossil(options: FossilOptions): Extension {
  return [
    fossilHighlighting(
      { tokenize: options.tokenize, tokenKinds: options.tokenKinds },
      { maxLength: options.maxLength },
    ),
    fossilLinter(options.check, {
      uri: options.uri,
      delay: options.delay,
      onDiagnostics: options.onDiagnostics,
    }),
  ];
}
