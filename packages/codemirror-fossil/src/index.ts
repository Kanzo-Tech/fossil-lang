/**
 * `@fossil-lang/codemirror-fossil` — the fossil language layer for CodeMirror 6.
 *
 * Five halves by now, and every one of them is an answer the compiler already
 * had:
 *
 * - **Highlighting**, from `tokenize()` + `tokenKinds()` — the compiler's own
 *   lexer, so no editor reimplements the grammar in TypeScript and drifts.
 * - **Diagnostics**, from `check()` — `CheckRow`s with UTF-16 spans, projected
 *   onto `@codemirror/lint`.
 * - **Hover**, from `hover()` — the type of what you wrote AND the type the
 *   target shape demands of it, which is the pair that makes hovering worth
 *   anything in a mapping language.
 * - **Completion**, from `completions()` — narrowed by the receiver's type
 *   rather than ranked over a catalogue.
 * - **Go to definition**, from `gotoDefinition()` — and two of the four
 *   positions it recognises resolve into the *shape document*, so the host is
 *   handed a cross-file target rather than a silent no-op.
 *
 * ## It composes into an editor rather than being one
 *
 * Everything here is an `Extension`. There is no component, no `EditorView`
 * construction, and no theme — one `baseTheme` sets the margins of the hover
 * tooltip's own markup and not a single colour, at the lowest precedence
 * CodeMirror has. `@kanzo-tech/ui`'s `CodeEditor` holds exactly this in a
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
 * const sync = (text: string) => { pg.updateFile(handle, text); };
 *
 * const extensions = fossil({
 *   tokenize,
 *   tokenKinds,
 *   uri: 'hello.fossil',
 *   check: (text) => { sync(text); return pg.check(); },
 *   hover: (text, line, ch) => { sync(text); return pg.hover(handle, line, ch); },
 *   complete: (text, line, ch) => { sync(text); return pg.completions(handle, line, ch); },
 *   definition: (text, line, ch) => { sync(text); return pg.gotoDefinition(handle, line, ch); },
 *   onNavigate: (target) => report(target),
 * });
 * ```
 *
 * **Every source takes the text**, and that repetition is the design. The wasm
 * workspace answers about the text of the last `updateFile`; the checker is
 * debounced, hover fires on mouse-move and completion on nearly every keystroke,
 * so three of the four sources run between two checks. A source that took only a
 * position would let a host query text it had not pushed and get a range one
 * keystroke wrong. Handing the text in makes the push and the query one step —
 * the same argument `check` was already making, applied to the three that need it
 * more. What that push costs is the host's to decide: a string comparison against
 * what it last sent skips the call entirely in the common case.
 *
 * ## Why one workspace, and not one per rate
 *
 * They could have been separate — a second `FossilPlayground` for the position
 * queries would put hover on its own Salsa store and end the question. It would
 * also double the interning, double the memory, and answer from a *different*
 * revision than the squiggles the user is looking at, which is the defect it was
 * meant to avoid wearing a different hat. The re-entrancy that made sharing look
 * dangerous is gone at the root: `crates/fossil-wasm`'s three position methods
 * take a SHARED borrow because none of them mutates, and shared borrows nest, so
 * a hover during a live check returns an answer rather than an error. Only
 * `updateFile` takes the exclusive borrow, and that is the one path
 * `@codemirror/lint` already coalesces.
 *
 * ## What it still does not cover
 *
 * **Semantic highlighting.** `semanticLegend()` is exported and `fossil-ide`
 * knows a type from a binding from a column; `tokenize()` does not, which is why
 * `Ident` carries no tag in `tags.ts`. The legend is on the main thread but the
 * tokens themselves still only come back over the Worker.
 *
 * **Code actions.** `fossil-ide` has two quick fixes and both hang off a
 * diagnostic. `@codemirror/lint`'s `Diagnostic.actions` is the place they go, and
 * it needs the structured diagnostic the `CheckRow` wire form flattens.
 *
 * **Indentation.** Fossil is INDENT/DEDENT (`grammar.bnf`), so an `indentService`
 * would be genuinely useful and genuinely non-trivial — it needs the parser's view
 * of block openers, not the lexer's. Nothing here guesses at it.
 */
export { fossilHighlighting, buildDecorations } from './highlight.js';
export type { TokenSource, HighlightOptions } from './highlight.js';

export { fossilLinter, toDiagnostics } from './lint.js';
export type { CheckRowLike, CheckSource, LinterOptions } from './lint.js';

export { fossilHover, renderMarkdown } from './hover.js';
export type { HoverRowLike, HoverSource, HoverOptions } from './hover.js';

export { fossilCompletion, fossilCompletionSource, toCompletion } from './complete.js';
export type { CompletionRowLike, CompletionRowSource } from './complete.js';

export { fossilGotoDefinition, gotoDefinitionAt } from './navigate.js';
export type { DefinitionRowLike, DefinitionSource, NavigateOptions } from './navigate.js';

export { TAG_BY_NAME, tagFor } from './tags.js';
export { byteToUtf16Mapper } from './offsets.js';
export { offsetOf, positionOf, rangeOf } from './positions.js';
export type { Position, Range } from './positions.js';

import { type Extension } from '@codemirror/state';

import { fossilHighlighting, type HighlightOptions, type TokenSource } from './highlight.js';
import { fossilLinter, type CheckSource, type LinterOptions } from './lint.js';
import { fossilHover, type HoverOptions, type HoverSource } from './hover.js';
import { fossilCompletion, type CompletionRowSource } from './complete.js';
import {
  fossilGotoDefinition,
  type DefinitionRowLike,
  type DefinitionSource,
} from './navigate.js';

/** Everything {@link fossil} needs, in one object. */
export interface FossilOptions
  extends TokenSource,
    HighlightOptions,
    HoverOptions,
    Omit<LinterOptions, 'uri'> {
  /** The URI the buffer is open under in the workspace — the key `CheckRow.uri`
   *  carries, so rows for the OTHER open files (the `.shex` shape document, a
   *  second program) land on the right buffer, and so a definition target can be
   *  told from a jump. */
  uri: string;
  /** Run a check over the current text and return the workspace's rows.
   *
   *  The natural implementation updates the file and drains: `pg.updateFile(h,
   *  text); return pg.check();`. Doing both here rather than in two extensions is
   *  deliberate — the edit and the check are one step, and splitting them is how a
   *  host ends up checking text it has not yet pushed. */
  check: CheckSource;
  /** Answer a hover at a position. Omit for no hover. */
  hover?: HoverSource;
  /** Answer a completion request at a position. Omit for no completion. */
  complete?: CompletionRowSource;
  /** Answer a goto-definition at a position. Requires {@link onNavigate}. */
  definition?: DefinitionSource;
  /** Where a cross-file definition target goes, and where "nothing here" goes.
   *  Required alongside {@link definition} — a target in the shape document is
   *  the common case, not the exception, so an extension that silently dropped
   *  it would look broken most of the time. */
  onNavigate?: (target: DefinitionRowLike | null) => void;
}

/**
 * Every half the host asked for, as one extension.
 *
 * `tokenize`, `tokenKinds`, `uri` and `check` are required, because
 * highlighting without diagnostics is the thing this package exists to stop
 * being the only option. The three position sources are optional: a host that
 * has not wired the wasm workspace's position queries — or a read-only view that
 * does not want them — passes none and gets the two halves that were always
 * here.
 */
export function fossil(options: FossilOptions): Extension {
  const extensions: Extension[] = [
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

  if (options.hover) {
    extensions.push(
      fossilHover(options.hover, { hoverTime: options.hoverTime, render: options.render }),
    );
  }
  if (options.complete) extensions.push(fossilCompletion(options.complete));
  if (options.definition && options.onNavigate) {
    extensions.push(
      fossilGotoDefinition(options.definition, {
        uri: options.uri,
        onNavigate: options.onNavigate,
      }),
    );
  }

  return extensions;
}
