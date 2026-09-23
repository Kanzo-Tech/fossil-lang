/**
 * Go to definition — and the half of it that is not a cursor move.
 *
 * `fossil_ide::goto_definition` recognises four positions, and **two of them
 * leave the file**: a shape name in a mapping header resolves into the shape
 * document, and so does a property key. That is not an edge case in a mapping
 * language, it is the common one — the whole point is that the two ends of a
 * program are written in two files by two people. So this extension cannot
 * simply move a cursor: it moves the cursor when the target is in this buffer,
 * and hands the target to the host when it is not.
 *
 * A host with one editor pane (the playground) reports where the definition is.
 * A host with tabs opens one. Neither decision belongs in a language layer, and
 * guessing wrong would be worse than either — which is why `onNavigate` has no
 * default that opens anything.
 *
 * ## The keys
 *
 * `F12` is what every editor binds, and it is bound here. It is also **the
 * browser's**: in a tab, Chrome opens DevTools on F12 before the page sees the
 * event. So `Alt-.` is bound beside it — the M-. of every tags-based editor
 * since etags — and it is the one that works in a browser. Mod-click is bound
 * too, because a link is what this looks like.
 */
import { EditorView, keymap } from '@codemirror/view';
import { EditorSelection, type Extension } from '@codemirror/state';

import { offsetOf, positionOf, type Range } from './positions.js';

/** The `DefinitionRow` shape, restated structurally so this module imports no
 *  runtime. `@fossil-lang/wasm` is the definition. */
export interface DefinitionRowLike {
  /** The key the file was opened under, verbatim — NOT a `file://` URI. */
  uri: string;
  range: Range;
}

/** What {@link fossilGotoDefinition} calls. Takes the text for the same reason
 *  the check source does — see the note in `hover.ts`. */
export type DefinitionSource = (
  text: string,
  line: number,
  character: number,
) => readonly DefinitionRowLike[] | Promise<readonly DefinitionRowLike[]>;

/** Options for {@link fossilGotoDefinition}. */
export interface NavigateOptions {
  /** The URI this buffer is open under. A target carrying it is a jump; any
   *  other target is the host's to open. */
  uri: string;
  /**
   * Called with a target in ANOTHER file — and with `null` when the position
   * has no definition at all, so a host can say "nothing here" rather than
   * leave a keypress looking broken.
   *
   * Not optional: two of the four positions resolve across files, so an
   * extension without this would silently do nothing on the majority of real
   * uses.
   */
  onNavigate: (target: DefinitionRowLike | null) => void;
  /** Bind Mod-click as well as the keys. Default true. */
  clickToNavigate?: boolean;
}

/**
 * Jump, or hand over. Exported so a host can bind it to a key of its own.
 *
 * The first target wins when there are several — LSP's `Location[]` is ordered
 * by the server, and a picker is a host's UI decision rather than a language
 * layer's.
 */
export function gotoDefinitionAt(
  view: EditorView,
  pos: number,
  source: DefinitionSource,
  options: NavigateOptions,
): void {
  const { line, character } = positionOf(view.state, pos);
  void (async () => {
    let rows: readonly DefinitionRowLike[];
    try {
      rows = await source(view.state.doc.toString(), line, character);
    } catch {
      options.onNavigate(null);
      return;
    }
    const target = rows[0];
    if (target === undefined) {
      options.onNavigate(null);
      return;
    }
    if (target.uri !== options.uri) {
      options.onNavigate(target);
      return;
    }
    const at = offsetOf(view.state, target.range.start);
    view.dispatch({
      selection: EditorSelection.cursor(at),
      // `center` rather than `nearest`: a jump that scrolls the target to the
      // very bottom line reads as not having moved.
      effects: EditorView.scrollIntoView(at, { y: 'center' }),
      scrollIntoView: false,
    });
    view.focus();
  })();
}

/**
 * The goto-definition extension: `F12`, `Alt-.`, and Mod-click.
 */
export function fossilGotoDefinition(
  source: DefinitionSource,
  options: NavigateOptions,
): Extension {
  const atCursor = (view: EditorView): boolean => {
    gotoDefinitionAt(view, view.state.selection.main.head, source, options);
    return true;
  };

  const extensions: Extension[] = [
    keymap.of([
      { key: 'F12', run: atCursor },
      { key: 'Alt-.', run: atCursor },
    ]),
  ];

  if (options.clickToNavigate !== false) {
    extensions.push(
      EditorView.domEventHandlers({
        mousedown(event, view) {
          if (!(event.metaKey || event.ctrlKey) || event.button !== 0) return false;
          const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
          if (pos === null) return false;
          event.preventDefault();
          gotoDefinitionAt(view, pos, source, options);
          return true;
        },
      }),
    );
  }

  return extensions;
}
