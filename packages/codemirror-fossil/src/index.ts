/**
 * `@fossil-lang/codemirror-fossil` — the fossil language layer for CodeMirror 6.
 *
 * Five halves by now, and every one of them is an answer the compiler already
 * had:
 *
 * - **Highlighting**, from `tokenize()` — the compiler's own
 *   lexer, so no editor reimplements the grammar in TypeScript and drifts — with
 *   `semanticTokens()` laid over it: shapes, declarations and connections, which
 *   the lexer cannot tell apart.
 * - **Diagnostics**, from `diagnostics()` — LSP-shaped, with UTF-16 spans, projected
 *   onto `@codemirror/lint`, with `fossil-ide`'s two quick fixes — the row's
 *   `didYouMean` and `suggestion` — as the diagnostic's actions.
 * - **Hover**, from `hover()` — the type of what you wrote AND the type the
 *   target shape demands of it, which is the pair that makes hovering worth
 *   anything in a mapping language.
 * - **Completion**, from `completion()` — narrowed by the receiver's type
 *   rather than ranked over a catalogue.
 * - **Go to definition**, from `definition()` — and two of the four
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
 * import { openProgram } from '@fossil-lang/wasm';
 *
 * const program = await openProgram('hello.fossil', { host, text });
 * const extensions = fossil(program, { onNavigate: (target) => report(target) });
 * ```
 *
 * ## The view pushes the text; every question takes only a position
 *
 * A `ViewPlugin` calls `program.update(text)` once at construction and on every `docChanged` —
 * `@codemirror/lsp-client`'s `syncFiles`, and LSP's `didChange`. A plugin's `update` runs
 * synchronously inside the `dispatch` that changed the document, before any hover, completion or
 * lint source can read the new state, so every question asks by `(line, character)` about exactly
 * the text on screen. The program compares against what it holds, so a dispatch that changed
 * nothing it was told costs a string comparison. Hover, completion, definition and the linter are
 * extensions of the view rather than calls a host makes, which is why no host can ask about text
 * it has not pushed.
 *
 * ## A check that failed is a diagnostic, not a throw
 *
 * A check that throws — a document the host never answered for, a module that would not boot, a
 * worker that died — is drawn on the first character by its code, and it reaches `onDiagnostics`
 * as one diagnostic, {@link uncheckedDiagnostic}, through the same call as every successful batch.
 * A host that renders its own panel or a "Valid" badge from `onDiagnostics` therefore needs no
 * `try` around `diagnostics` to learn the check failed, and no copy of it to say so: a failure
 * before there is a linter at all — `openProgram` rejecting — is reported with the same
 * `uncheckedDiagnostic(uri, cause)`.
 *
 * ## Why one workspace, and not one per rate
 *
 * They could have been separate — a second workspace for the position
 * queries would put hover on its own Salsa store and end the question. It would
 * also double the interning, double the memory, and answer from a *different*
 * revision than the squiggles the user is looking at, which is the defect it was
 * meant to avoid wearing a different hat. The re-entrancy that made sharing look
 * dangerous is gone at the root: `crates/fossil-wasm`'s position methods
 * take a SHARED borrow because none of them mutates, and shared borrows nest, so
 * a hover during a live check returns an answer rather than an error. Only
 * `update` takes the exclusive borrow.
 *
 * ## What it still does not cover
 *
 *
 * **Indentation.** Fossil is INDENT/DEDENT (`grammar.bnf`), so an `indentService`
 * would be genuinely useful and genuinely non-trivial — it needs the parser's view
 * of block openers, not the lexer's. Nothing here guesses at it.
 */
import { ViewPlugin } from '@codemirror/view';
import type { Extension } from '@codemirror/state';
import type { Location } from '@fossil-lang/types';
import type { FossilProgram } from '@fossil-lang/wasm';

import { fossilCompletion } from './complete.js';
import { fossilHighlighting, type HighlightOptions } from './highlight.js';
import { fossilHover, type HoverOptions } from './hover.js';
import { fossilLinter, type LinterOptions } from './lint.js';
import { fossilGotoDefinition } from './navigate.js';

export { uncheckedDiagnostic } from './lint.js';

/** What {@link fossil} takes beside the program — each one the host's to decide. */
export interface FossilOptions extends HighlightOptions, HoverOptions, LinterOptions {
  /**
   * Where a cross-file definition target goes, and where "nothing here" goes. Go to definition is
   * bound only when it is given: a target in the shape document is the common case, not the
   * exception, so an extension that silently dropped it would look broken most of the time.
   */
  onNavigate?: (target: Location | null) => void;
}

/**
 * The fossil language layer for one program, as one extension: the sync, highlighting,
 * diagnostics, hover, completion and — with `onNavigate` — go to definition.
 */
export function fossil(program: FossilProgram, options: FossilOptions = {}): Extension {
  const { onNavigate } = options;
  return [
    // First, so it runs before every other plugin's `update` in the same dispatch: view plugins
    // update in the order their extensions appear.
    ViewPlugin.define((view) => {
      program.update(view.state.doc.toString());
      return {
        update(update) {
          if (update.docChanged) program.update(update.state.doc.toString());
        },
      };
    }),
    fossilHighlighting(program, options),
    fossilLinter(program, options),
    fossilHover(program, options),
    fossilCompletion(program),
    ...(onNavigate === undefined ? [] : [fossilGotoDefinition(program, { onNavigate })]),
  ];
}
